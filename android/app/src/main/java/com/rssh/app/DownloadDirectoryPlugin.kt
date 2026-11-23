package com.rssh.app

import android.app.Activity
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.database.Cursor
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import android.os.Process
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.webkit.MimeTypeMap
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.IOException
import java.nio.file.FileAlreadyExistsException
import java.util.concurrent.Executors

private const val READ_WRITE = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
private const val UNSUPPORTED = "sftp_folder_operations_unsupported"
private const val ACCESS = "sftp_provider_siblings_unavailable"

private class UnsupportedFolder(message: String = "Unsupported document provider") : Exception(message)
private class NameConflict : Exception()

/** Directory grants last for this activity's task, including in-process transfer retries. */
@TauriPlugin
class DownloadDirectoryPlugin(activity: Activity) : Plugin(activity) {
    private val context = activity.applicationContext
    private val io = Executors.newSingleThreadExecutor()
    private val writers = mutableMapOf<Pair<Uri, String>, ParcelFileDescriptor>()
    private var picking = false

    @Command
    fun pickDirectory(invoke: Invoke) {
        // Tauri has one pending activity-result callback.
        if (picking) {
            invoke.reject("A folder picker is already open", "sftp_folder_picker_busy")
            return
        }
        try {
            picking = true
            startActivityForResult(
                invoke, Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(READ_WRITE), "picked"
            )
        } catch (e: Exception) {
            picking = false
            invoke.reject(e.message, "sftp_folder_picker_unavailable")
        }
    }

    @ActivityCallback
    fun picked(invoke: Invoke, result: ActivityResult) {
        picking = false
        if (result.resultCode == Activity.RESULT_CANCELED) {
            invoke.resolve()
            return
        }
        work(invoke) {
            val data = result.data
            if (result.resultCode != Activity.RESULT_OK || data == null || (data.flags and READ_WRITE) != READ_WRITE) {
                throw SecurityException()
            }
            val tree = data.data ?: throw SecurityException()
            val directory = LocalDirectory(context, tree)
            directory.child(invoke.getArgs().getString("name"))
            invoke.resolve(JSObject().put("tree", tree.toString()).put("folder", directory.displayName))
        }
    }

    @Command
    fun fileOperation(invoke: Invoke) = work(invoke) {
        val args = invoke.getArgs()
        val tree = Uri.parse(args.getString("tree"))
        val name = args.getString("name")
        val op = args.getString("op")
        val key = tree to name
        if (op == "closeWriter") {
            // Closing a previously opened descriptor still works if the grant expired.
            writers.remove(key)?.use { if (it.canDetectErrors()) it.checkError() }
            invoke.resolve()
            return@work
        }
        val directory = LocalDirectory(context, tree)
        when (op) {
            "snapshot", "snapshotWritable" -> {
                val file = directory.child(name)
                if (file == null) invoke.resolve()
                else directory.view(file, if (op == "snapshot") "r" else "rw").use {
                    invoke.resolveObject(it.dup().detachFd())
                }
            }
            "open", "create" -> {
                if (writers.containsKey(key)) throw IOException("A writer is still closing: $name")
                directory.openWriter(name, create = op == "create") { original ->
                    // Keep the provider's reliable writer until Rust closes its stream.
                    val fd = original.dup().detachFd()
                    try {
                        writers[key] = original
                        invoke.resolveObject(fd)
                    } catch (e: Exception) {
                        writers.remove(key)
                        ParcelFileDescriptor.adoptFd(fd).close()
                        throw e
                    }
                }
            }
            "remove" -> {
                directory.remove(name)
                invoke.resolve()
            }
            "rename" -> {
                directory.rename(name, args.getString("to"), args.getBoolean("allowExisting"), args.getBoolean("staged"))
                invoke.resolve()
            }
            else -> throw IllegalArgumentException("Unknown file operation")
        }
    }

    private fun work(invoke: Invoke, op: () -> Unit) {
        io.execute {
            try {
                op()
            } catch (e: SecurityException) {
                invoke.reject(e.message ?: "Folder grant unavailable", ACCESS)
            } catch (e: UnsupportedFolder) {
                invoke.reject(e.message, UNSUPPORTED)
            } catch (e: NameConflict) {
                invoke.reject("Document already exists", "already_exists")
            } catch (e: Exception) {
                invoke.reject(e.message ?: e.javaClass.simpleName)
            }
        }
    }
}

internal class LocalDirectory(private val context: Context, private val tree: Uri) {
    private val resolver: ContentResolver = context.contentResolver
    private val parent: Uri
    val displayName: String

    private val columns = arrayOf(
        Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE, Document.COLUMN_FLAGS
    )

    init {
        if (tree.scheme != ContentResolver.SCHEME_CONTENT || !DocumentsContract.isTreeUri(tree) ||
            tree.pathSegments.size != 2 || context.checkUriPermission(
                tree, Process.myPid(), Process.myUid(), READ_WRITE
            ) != PackageManager.PERMISSION_GRANTED
        ) throw SecurityException()
        parent = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
        displayName = resolver.query(parent, columns, null, null, null).use {
            if (it == null || !it.moveToFirst() || it.getString(2) != Document.MIME_TYPE_DIR ||
                (it.getInt(3) and Document.FLAG_DIR_SUPPORTS_CREATE) == 0
            ) throw UnsupportedFolder()
            it.getString(1)
        }
    }

    data class Child(val uri: Uri, val flags: Int) {
        fun require(flag: Int) {
            if ((flags and flag) != flag) throw UnsupportedFolder()
        }
    }

    fun child(name: String, regular: Boolean = true): Child? {
        if (name.isEmpty() || name == "." || name == ".." || '/' in name || '\u0000' in name) throw UnsupportedFolder()
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
        return resolver.query(children, columns, null, null, null).use { rows ->
            if (rows == null || rows.extras.getBoolean(DocumentsContract.EXTRA_LOADING) ||
                rows.extras.containsKey(DocumentsContract.EXTRA_ERROR)) throw IOException("Incomplete directory listing")
            var found: Child? = null
            while (rows.moveToNext()) {
                val display = rows.getString(1)
                if (!display.equals(name, ignoreCase = true)) continue
                if (found != null || display != name) throw UnsupportedFolder()
                if (regular && rows.getString(2) == Document.MIME_TYPE_DIR) throw IOException("Is a directory: $name")
                if (regular && (rows.getInt(3) and Document.FLAG_VIRTUAL_DOCUMENT) != 0) throw UnsupportedFolder()
                found = Child(DocumentsContract.buildDocumentUriUsingTree(tree, rows.getString(0)), rows.getInt(3))
            }
            found
        }
    }

    fun descriptor(file: Child, mode: String): ParcelFileDescriptor {
        if (mode == "rw") file.require(Document.FLAG_SUPPORTS_WRITE)
        val pfd = resolver.openFileDescriptor(file.uri, mode) ?: throw IOException("Could not open document")
        try {
            val stat = Os.fstat(pfd.fileDescriptor)
            if (!OsConstants.S_ISREG(stat.st_mode) || stat.st_ino == 0L) throw UnsupportedFolder()
            Os.lseek(pfd.fileDescriptor, 0, OsConstants.SEEK_SET)
            return pfd
        } catch (e: Exception) {
            pfd.close()
            if (e is ErrnoException && e.errno == OsConstants.ESPIPE) throw UnsupportedFolder()
            throw e
        }
    }

    fun view(file: Child, mode: String): ParcelFileDescriptor = try {
        descriptor(file, mode)
    } catch (e: UnsupportedFolder) {
        // SAF allows an "r" view to be a stream even when "rw" is seekable.
        if (mode != "r") throw e
        descriptor(file, "rw")
    }

    fun openWriter(name: String, create: Boolean, accept: (ParcelFileDescriptor) -> Unit) {
        if (!create) {
            val file = child(name) ?: throw IOException("Document missing: $name")
            val original = descriptor(file, "rw")
            try {
                accept(original)
            } catch (e: Exception) {
                original.close()
                throw e
            }
            return
        }
        if (child(name, regular = false) != null) throw NameConflict()
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substringAfterLast('.'))
            ?: "application/octet-stream"
        val created = DocumentsContract.createDocument(resolver, parent, mime, name)
            ?: throw IOException("Creation was not confirmed at $displayName/$name")
        var writer: ParcelFileDescriptor? = null
        var initial: Identity? = null
        var location = created.toString()
        try {
            // Identify the document actually returned, including a provider-numbered name.
            val actual = displayName(created)
            location = "$displayName/$actual"
            val file = child(actual)?.takeIf { it.uri == created } ?: throw IOException("Document identity changed")
            writer = descriptor(file, "rw")
            initial = identity(writer)
            if (actual != name) throw UnsupportedFolder("Provider changed the document name")
            file.require(Document.FLAG_SUPPORTS_RENAME or Document.FLAG_SUPPORTS_DELETE)
            accept(writer)
        } catch (failure: Exception) {
            try {
                // A path-based document ID can now name a replacement. Keep the
                // original inode pinned while checking what the URI resolves to.
                writer.use {
                    if (initial == null) throw IOException("Could not identify the created document")
                    val actual = displayName(created)
                    location = "$displayName/$actual"
                    val current = child(actual)
                    if (current == null || current.uri != created || identity(current, "rw") != initial) {
                        throw IOException("Document identity changed")
                    }
                    if (!DocumentsContract.deleteDocument(resolver, created)) throw IOException("Deletion was not confirmed")
                }
            } catch (cleanup: Exception) {
                throw IOException("Could not confirm removal of $location: ${cleanup.message}. ${failure.message}", failure)
            }
            throw failure
        }
    }

    fun remove(name: String) {
        val file = child(name) ?: throw IOException("Document missing: $name")
        file.require(Document.FLAG_SUPPORTS_DELETE)
        if (!DocumentsContract.deleteDocument(resolver, file.uri)) throw IOException("Deletion was not confirmed")
    }

    private data class Identity(val dev: Long, val ino: Long, val size: Long, val modified: Long)

    private fun identity(file: Child, mode: String): Identity = view(file, mode).use(::identity)

    private fun identity(descriptor: ParcelFileDescriptor): Identity {
        val stat = Os.fstat(descriptor.fileDescriptor)
        if (!OsConstants.S_ISREG(stat.st_mode) || stat.st_ino == 0L) throw UnsupportedFolder()
        return Identity(stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime)
    }

    // Once the provider has returned a new URI, preserve that location even
    // if the subsequent query fails or the folder grant is revoked.
    private fun <T> verifyAt(location: String, check: () -> T): T = try {
        check()
    } catch (e: Exception) {
        throw IOException("Could not verify $location: ${e.message}", e)
    }

    private fun displayName(uri: Uri): String = verifyAt(uri.toString()) {
        resolver.query(uri, arrayOf(Document.COLUMN_DISPLAY_NAME), null, null, null).use {
            if (it == null || !it.moveToFirst()) throw IOException("Missing document")
            it.getString(0)
        }
    }

    private fun isConflict(e: Throwable): Boolean = generateSequence(e) { it.cause }.take(8).any {
        (it is ErrnoException && it.errno == OsConstants.EEXIST) ||
            (Build.VERSION.SDK_INT >= 26 && it is FileAlreadyExistsException) ||
            (it is IllegalStateException && it.message?.startsWith("Already exists ") == true)
    }

    private fun numberedName(requested: String, actual: String): Boolean {
        val dot = requested.lastIndexOf('.').let { if (it == -1) requested.length else it }
        val base = Regex.escape(requested.substring(0, dot))
        val extension = Regex.escape(requested.substring(dot))
        return Regex("$base \\([1-9][0-9]*\\)$extension").matches(actual)
    }

    fun rename(from: String, to: String, allowExisting: Boolean, staged: Boolean) {
        val file = child(from) ?: throw IOException("Document missing: $from")
        file.require(Document.FLAG_SUPPORTS_RENAME)
        val original = child(to, regular = allowExisting)
        if (original != null && !allowExisting) throw NameConflict()
        val mode = if (staged) "rw" else "r"
        val before = identity(file, mode)
        val previous = original?.let { identity(it, "r") }
        val moved = try {
            DocumentsContract.renameDocument(resolver, file.uri, to)
                ?: throw IOException("Rename was not confirmed")
        } catch (e: Exception) {
            if (isConflict(e) && child(from)?.let { identity(it, mode) } == before) {
                val occupied = child(to, regular = previous != null)
                if (occupied != null && (previous == null || identity(occupied, "r") == previous)) throw NameConflict()
            }
            throw e
        }
        val actual = displayName(moved)
        // Newer system providers append a numeric suffix on conflict. Undo only
        // our own staged file if the original target and current identities agree.
        val retained = "$displayName/$actual"
        verifyAt(retained) {
            val fileAtName = child(actual)
            if (fileAtName == null || fileAtName.uri != moved || identity(fileAtName, mode) != before) {
                throw IOException("Document identity changed")
            }
        }
        if (actual == to) return
        if (!allowExisting || previous == null) throw IOException("Unexpected rename result at $retained")
        verifyAt(retained) {
            if (child(from) != null || child(to)?.let { identity(it, "r") } != previous) {
                throw IOException("Destination changed during rename")
            }
        }
        val restored = try {
            DocumentsContract.renameDocument(resolver, moved, from)
                ?: throw IOException("Restore was not confirmed")
        } catch (e: Exception) {
            throw IOException("Check $retained and $displayName/$from", e)
        }
        val restoredName = displayName(restored)
        verifyAt("$displayName/$restoredName") {
            val stagedFile = child(from)
            if (restoredName != from || stagedFile == null || stagedFile.uri != restored ||
                identity(stagedFile, mode) != before || child(to)?.let { identity(it, "r") } != previous
            ) throw IOException("Unexpected restore result")
        }
        if (!numberedName(to, actual)) {
            throw UnsupportedFolder("Provider changed $to to $actual; the staged file was restored to $displayName/$from")
        }
        throw NameConflict()
    }
}
