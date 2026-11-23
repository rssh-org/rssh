package com.rssh.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

@InvokeArg
class DirectoryArgs {
    lateinit var parent: String
    var name: String = ""
    var part: String = ""
    var scope: String = ""
}

private data class Child(val uri: Uri, val name: String, val type: String, val flags: Int, val size: Long?, val modified: Long?)
internal class DirectoryError(val code: String, detail: String, val path: String = "") : Exception(detail)

// A document already exists on the provider before it supplies a usable descriptor.
internal fun <T> prepareCreatedDocument(path: String, prepare: () -> T, discard: () -> Unit): T {
    try {
        return prepare()
    } catch (error: Exception) {
        try {
            discard()
        } catch (cleanup: Exception) {
            throw DirectoryError("sftp_staging_retained", "${error.message}. ${cleanup.message}", path)
        }
        throw error
    }
}

@TauriPlugin
class DownloadDirectoryPlugin(activity: Activity) : Plugin(activity) {
    private val resolver = activity.applicationContext.contentResolver
    private val workers = Executors.newCachedThreadPool()
    private val grants = ConcurrentHashMap.newKeySet<String>()
    private val directories = ConcurrentHashMap.newKeySet<String>()
    private val descriptors = ConcurrentHashMap<String, ParcelFileDescriptor>()
    private val projection = arrayOf(Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME,
        Document.COLUMN_MIME_TYPE, Document.COLUMN_FLAGS, Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED)

    @Command
    fun pick(invoke: Invoke) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        startActivityForResult(invoke, intent, "picked")
    }

    @ActivityCallback
    fun picked(invoke: Invoke, result: ActivityResult) {
        val data = result.data
        val tree = data?.data
        if (result.resultCode == Activity.RESULT_CANCELED) { invoke.resolve(JSObject()); return }
        val required = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        if (result.resultCode != Activity.RESULT_OK || tree == null || !DocumentsContract.isTreeUri(tree) ||
            (data.flags and required) != required) {
            invoke.resolve(JSObject().put("error", "sftp_directory_required"))
            return
        }
        grants.add(tree.toString())
        invoke.resolve(JSObject().put("uri", tree.toString()))
    }

    private fun operation(invoke: Invoke, block: (DirectoryArgs) -> JSObject) {
        val args = invoke.parseArgs(DirectoryArgs::class.java)
        workers.execute {
            try { invoke.resolve(block(args)) }
            catch (e: Exception) {
                val result = JSObject()
                result.put("error", when (e) {
                    is DirectoryError -> e.code
                    is SecurityException -> "sftp_directory_required"
                    else -> "sftp_directory_unsupported"
                })
                result.put("detail", e.message ?: e.javaClass.simpleName)
                result.put("path", (e as? DirectoryError)?.path ?: "")
                invoke.resolve(result)
            }
        }
    }

    private fun valid(name: String) = name.isNotEmpty() && name != "." && name != ".." && '/' !in name && '\u0000' !in name
    // Android's FileSystemProvider uses "stem (n).ext" for a name collision.
    // Other alternate names may be sanitization or an enforced MIME extension.
    private fun collisionName(requested: String, actual: String): Boolean {
        val dot = requested.lastIndexOf('.').takeIf { it > 0 } ?: requested.length
        val prefix = requested.take(dot) + " ("
        val suffix = ")" + requested.drop(dot)
        if (!actual.startsWith(prefix) || !actual.endsWith(suffix) || actual.length <= prefix.length + suffix.length) return false
        return actual.substring(prefix.length, actual.length - suffix.length).toIntOrNull()?.let { it > 0 } == true
    }

    private fun registered(value: String): Uri {
        if (!directories.contains(value)) throw DirectoryError("sftp_directory_required", "Directory access expired")
        return Uri.parse(value)
    }
    private fun rows(parent: Uri, uri: Uri): List<Child> {
        val cursor = resolver.query(uri, projection, null, null, null) ?: error("Provider returned no listing")
        return cursor.use {
            check(!it.extras.getBoolean(DocumentsContract.EXTRA_LOADING)) { "Provider directory is still loading" }
            check(it.extras.getString(DocumentsContract.EXTRA_ERROR) == null) { "Provider directory query failed" }
            buildList {
                while (it.moveToNext()) add(Child(DocumentsContract.buildDocumentUriUsingTree(parent, it.getString(0)),
                    it.getString(1), it.getString(2), it.getInt(3), if (it.isNull(4)) null else it.getLong(4),
                    if (it.isNull(5)) null else it.getLong(5)))
            }
        }
    }
    private fun child(parent: Uri, name: String): Child? {
        require(valid(name))
        val listing = DocumentsContract.buildChildDocumentsUriUsingTree(parent, DocumentsContract.getDocumentId(parent))
        val matches = rows(parent, listing).filter { it.name == name }
        check(matches.size <= 1) { "Ambiguous document name: $name" }
        return matches.firstOrNull()
    }
    private fun file(entry: Child): Child {
        check(entry.type != Document.MIME_TYPE_DIR && (entry.flags and Document.FLAG_VIRTUAL_DOCUMENT) == 0) {
            "Expected a regular file: ${entry.name}"
        }
        return entry
    }
    private fun rename(entry: Child, name: String): Child {
        check((entry.flags and Document.FLAG_SUPPORTS_RENAME) != 0) { "Provider cannot rename this document" }
        val uri = DocumentsContract.renameDocument(resolver, entry.uri, name) ?: error("Rename returned no document URI")
        return rows(uri, uri).single()
    }

    private fun openPart(entry: Child, scope: String): JSObject {
        require(scope.isNotEmpty())
        val descriptor = resolver.openFileDescriptor(file(entry).uri, "rw")
            ?: error("Provider returned no file descriptor")
        try {
            val fd = descriptor.fileDescriptor
            check(OsConstants.S_ISREG(Os.fstat(fd).st_mode)) { "Provider must supply a regular file" }
            Os.lseek(fd, 0, OsConstants.SEEK_SET)
            // Some providers require a close notification to commit writes. Keep their
            // descriptor until Rust has closed its duplicate and requests release.
            check(descriptors.putIfAbsent(scope, descriptor) == null) { "Staging descriptor is still open" }
            return JSObject().put("fd", descriptor.dup().detachFd())
        } catch (error: Exception) {
            descriptors.remove(scope, descriptor)
            descriptor.close()
            throw error
        }
    }
    @Command
    fun release(invoke: Invoke) = operation(invoke) { args ->
        descriptors.remove(args.scope)?.close()
        JSObject()
    }
    @Command
    fun directory(invoke: Invoke) = operation(invoke) { args ->
        if (!grants.contains(args.parent)) throw DirectoryError("sftp_directory_required", "Directory access expired")
        val tree = Uri.parse(args.parent)
        var parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
        if (args.name.isNotEmpty()) for (name in args.name.split('/')) {
            require(valid(name))
            val next = child(parent, name) ?: run {
                val uri = DocumentsContract.createDocument(resolver, parent, Document.MIME_TYPE_DIR, name)
                    ?: error("Provider could not create directory")
                rows(parent, uri).single()
            }
            check(next.type == Document.MIME_TYPE_DIR && next.name == name) { "Invalid destination directory" }
            parent = next.uri
        }
        val attrs = rows(parent, parent).single()
        check((attrs.flags and Document.FLAG_DIR_SUPPORTS_CREATE) != 0) { "Directory does not allow creating files" }
        directories.add(parent.toString())
        JSObject().put("uri", parent.toString()).put("identity",
            "${parent.authority}: ${DocumentsContract.getDocumentId(parent)}")
    }

    @Command
    fun inspect(invoke: Invoke) = operation(invoke) { args ->
        val entry = child(registered(args.parent), args.name)?.let(::file)
        JSObject().apply {
            if (entry != null) {
                // Some providers omit the modification time. Identify the content before
                // accepting an existing document as the same destination on a retry.
                val revision = entry.modified?.takeIf { it > 0 }?.toString() ?: run {
                    val hash = MessageDigest.getInstance("SHA-256")
                    val input = resolver.openInputStream(entry.uri) ?: error("Cannot verify existing document")
                    input.use { stream ->
                        val buffer = ByteArray(64 * 1024)
                        var count = stream.read(buffer)
                        while (count > 0) {
                            hash.update(buffer, 0, count)
                            count = stream.read(buffer)
                        }
                        check(count == -1) { "Provider did not return the complete document" }
                    }
                    android.util.Base64.encodeToString(hash.digest(), android.util.Base64.NO_WRAP)
                }
                put("identity", "${DocumentsContract.getDocumentId(entry.uri)}\n$revision")
                put("size", entry.size)
            }
        }
    }
    @Command
    fun reopen(invoke: Invoke) = operation(invoke) { args ->
        openPart(child(registered(args.parent), args.name) ?: error("Staging document disappeared"), args.scope)
    }
    @Command
    fun create(invoke: Invoke) = operation(invoke) { args ->
        val parent = registered(args.parent)
        check(child(parent, args.name) == null) { "Staging name already exists" }
        val uri = DocumentsContract.createDocument(resolver, parent, "application/octet-stream", args.name)
            ?: error("Provider could not create staging document")
        var created: Child? = null
        prepareCreatedDocument("${DocumentsContract.getDocumentId(parent)}/${args.name} ($uri)", prepare = {
            val entry = rows(parent, uri).single()
            created = entry
            check(entry.name == args.name && (entry.flags and Document.FLAG_SUPPORTS_RENAME) != 0 &&
                (entry.flags and Document.FLAG_SUPPORTS_DELETE) != 0) {
                "Provider does not support exact staging names, rename and delete"
            }
            openPart(entry, args.scope)
        }, discard = {
            val entry = created ?: error("Could not identify the new staging document")
            check(entry.uri == uri && child(parent, entry.name) == entry &&
                (entry.flags and Document.FLAG_SUPPORTS_DELETE) != 0 &&
                DocumentsContract.deleteDocument(resolver, uri)) { "Could not remove the new staging document" }
        })
    }
    @Command
    fun publish(invoke: Invoke) = operation(invoke) { args ->
        val parent = registered(args.parent)
        val part = file(child(parent, args.part) ?: error("Staging document disappeared"))
        val old = child(parent, args.name)?.let(::file)
        try {
            val moved = rename(part, args.name)
            if (moved.name == args.name) {
                check(child(parent, args.name)?.uri == moved.uri) { "Provider returned a duplicate final name" }
                return@operation JSObject()
            }
            // SAF can resolve collisions by actually moving the staging document
            // to an alternate name. Verify both documents and move our file back
            // to its exact staging name before touching the original.
            if (!collisionName(args.name, moved.name) || old == null || child(parent, args.name) != old ||
                child(parent, args.part) != null) {
                throw DirectoryError("sftp_replace_uncertain", "Provider changed the requested name", moved.uri.toString())
            }
            val restored = rename(moved, args.part)
            check(restored.name == args.part && child(parent, args.part)?.uri == restored.uri)
        } catch (e: DirectoryError) {
            throw e
        } catch (e: Exception) {
            // A generic provider failure is insufficient evidence of a collision.
            val collision = generateSequence<Throwable>(e) { it.cause }
                .any { it is ErrnoException && it.errno == OsConstants.EEXIST }
            if (!collision || old == null || child(parent, args.name) != old || child(parent, args.part) != part)
                throw DirectoryError("sftp_replace_uncertain", e.message ?: "Provider rename failed", part.uri.toString())
        }
        check(old != null && child(parent, args.name) == old)
        val staged = file(child(parent, args.part) ?: error("Staging document disappeared"))
        val backupName = args.part.removeSuffix(".part") + ".bak"
        check(child(parent, backupName) == null)
        val folder = DocumentsContract.getDocumentId(parent)
        val backup = try { rename(old, backupName) } catch (e: Exception) {
            throw DirectoryError("sftp_replace_uncertain", e.message ?: "Backup rename failed", "$folder/$backupName (parent: $parent)")
        }
        try {
            check(backup.name == backupName && child(parent, backupName)?.uri == backup.uri)
            check(child(parent, args.name) == null) { "Destination appeared during replacement" }
            val final = rename(staged, args.name)
            check(final.name == args.name && child(parent, args.name)?.uri == final.uri) { "Provider did not publish the exact name" }
        } catch (e: Exception) {
            throw DirectoryError("sftp_replace_backup", e.message ?: "Could not finish replacement", "$folder/${backup.name} (${backup.uri})")
        }
        try {
            check(child(parent, backupName) == backup && DocumentsContract.deleteDocument(resolver, backup.uri)) {
                "Provider did not remove the backup"
            }
        } catch (e: Exception) {
            throw DirectoryError("sftp_backup_cleanup", e.message ?: "Backup cleanup failed", "$folder/${backup.name} (${backup.uri})")
        }
        JSObject()
    }
}
