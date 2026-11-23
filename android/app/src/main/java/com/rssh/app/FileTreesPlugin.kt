package com.rssh.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract as Docs
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONArray
import java.util.UUID
import java.util.concurrent.Executors

@InvokeArg
class TreeArgs {
    var op: String = ""
    var path: String = ""
    var to: String? = null
    var write: Boolean = false
    var replace: Boolean = false
}

internal data class TreeGrant<T>(val uri: T, val write: Boolean)
internal class FolderAuthorizationException : SecurityException("Authorize the complete target folder for this operation")
internal fun <T> requireTreeGrant(grants: Map<String, TreeGrant<T>>, token: String, write: Boolean): TreeGrant<T> {
    val grant = grants[token] ?: throw FolderAuthorizationException()
    if (write && !grant.write) throw FolderAuthorizationException()
    return grant
}
private data class TreePath(val token: String, val grant: TreeGrant<Uri>, val parts: List<String>)
private data class Document(val uri: Uri, val id: String, val name: String, val mime: String, val flags: Long, val size: Long?, val modified: Long?)

internal data class RenameDocument(val uri: String, val id: String, val name: String, val size: Long?, val modified: Long?)
internal data class RenameResult(val conflict: Boolean = false, val error: String? = null, val recovery: String? = null)
internal interface DocumentRenameAccess {
    fun rename(uri: String, name: String): String?
    fun read(uri: String): RenameDocument?
    fun original(): RenameDocument?
    fun target(): RenameDocument?
}

/** A provider can change the document URI on either rename. Recovery means the
 * last URI confirmed by a returned result, not proof that a failed later call
 * left the document there. Never lose it to a subsequent metadata/query error. */
internal fun renameWithRecovery(source: RenameDocument, targetName: String, targetBefore: RenameDocument?, access: DocumentRenameAccess): RenameResult {
    var recovery = source.uri
    try {
        recovery = access.rename(source.uri, targetName)
            ?: throw IllegalStateException("Rename outcome is unknown")
        val renamed = access.read(recovery) ?: throw IllegalStateException("Rename outcome is unknown")
        if (renamed.name == targetName) {
            check(access.target()?.id == renamed.id) { "Renamed document identity is ambiguous" }
            return RenameResult()
        }
        // Some providers select another display name on conflict. Restore only
        // our staged document, and inspect both names before reporting conflict.
        check(access.original() == null) { "Provider changed the requested name and the staged name is occupied" }
        recovery = access.rename(recovery, source.name)
            ?: throw IllegalStateException("Restore outcome is unknown")
        val restored = access.read(recovery)
        val targetAfter = access.target()
        if (restored?.name == source.name && targetBefore != null && targetAfter?.id == targetBefore.id && targetAfter.size == targetBefore.size && targetAfter.modified == targetBefore.modified) {
            return RenameResult(conflict = true, error = "Provider does not replace an existing same-name document")
        }
        throw IllegalStateException("Provider does not preserve exact rename names or the target changed")
    } catch (error: Exception) {
        return RenameResult(error = error.message ?: error.javaClass.simpleName, recovery = recovery)
    }
}

private fun Document.renameDocument() = RenameDocument(uri.toString(), id, name, size, modified)

/** SAF grants and provider calls stay in the native host. Names remain literal;
 * IDs returned by renameDocument are never assumed to remain stable. */
@TauriPlugin
class FileTreesPlugin(private val host: Activity) : Plugin(host) {
    private val grants = java.util.concurrent.ConcurrentHashMap<String, TreeGrant<Uri>>()
    private val worker = Executors.newSingleThreadExecutor()
    private val resolver get() = host.contentResolver
    private val columns = arrayOf(Docs.Document.COLUMN_DOCUMENT_ID, Docs.Document.COLUMN_DISPLAY_NAME,
        Docs.Document.COLUMN_MIME_TYPE, Docs.Document.COLUMN_FLAGS, Docs.Document.COLUMN_SIZE, Docs.Document.COLUMN_LAST_MODIFIED)

    @Command
    fun pickFolder(invoke: Invoke) {
        val args = invoke.parseArgs(TreeArgs::class.java)
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or
            (if (args.write) Intent.FLAG_GRANT_WRITE_URI_PERMISSION else 0) or Intent.FLAG_GRANT_PREFIX_URI_PERMISSION)
        startActivityForResult(invoke, intent, "folderResult")
    }

    @ActivityCallback
    fun folderResult(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode != Activity.RESULT_OK) { invoke.resolve(JSObject()); return }
        try {
            val args = invoke.parseArgs(TreeArgs::class.java)
            val uri = result.data?.data ?: throw IllegalStateException("Missing folder authorization")
            val flags = result.data?.flags ?: 0
            check(flags and Intent.FLAG_GRANT_READ_URI_PERMISSION != 0) { "Read access to the complete folder is required" }
            if (args.write) check(flags and Intent.FLAG_GRANT_WRITE_URI_PERMISSION != 0) { "Write access to the complete folder is required" }
            val token = UUID.randomUUID().toString()
            grants[token] = TreeGrant(uri, args.write)
            invoke.resolve(JSObject().put("location", location(token, emptyList())))
        } catch (error: Exception) { invoke.reject(error.message ?: "Folder authorization failed") }
    }

    private fun location(token: String, parts: List<String>): String {
        val builder = Uri.Builder().scheme("rssh-tree").authority(token)
        parts.forEach { builder.appendPath(it) }
        return builder.build().toString()
    }
    private fun segments(value: String): List<String> = value.split('/').also { parts ->
        require(parts.all { it.isNotEmpty() && it != "." && it != ".." && !it.contains('\u0000') }) { "Invalid relative document name" }
    }
    private fun path(value: String, write: Boolean = false): TreePath {
        val uri = Uri.parse(value)
        if (uri.scheme != "rssh-tree") throw FolderAuthorizationException()
        val token = uri.authority ?: throw FolderAuthorizationException()
        val grant = requireTreeGrant(grants, token, write)
        val parts = uri.pathSegments
        require(parts.all { it.isNotEmpty() && it != "." && it != ".." && !it.contains('/') && !it.contains('\u0000') })
        return TreePath(token, grant, parts)
    }
    private fun read(uri: Uri): Document? {
        resolver.query(uri, columns, null, null, null)?.use { c ->
            if (!c.moveToFirst()) return null
            return Document(uri, c.getString(0), c.getString(1), c.getString(2), c.getLong(3),
                if (c.isNull(4)) null else c.getLong(4), if (c.isNull(5)) null else c.getLong(5))
        }
        throw IllegalStateException("Provider did not return document metadata")
    }
    private fun root(p: TreePath): Document = read(Docs.buildDocumentUriUsingTree(p.grant.uri, Docs.getTreeDocumentId(p.grant.uri)))
        ?: throw IllegalStateException("Authorized folder no longer exists")
    private fun children(p: TreePath, parent: Document): List<Document> {
        val query = Docs.buildChildDocumentsUriUsingTree(p.grant.uri, parent.id)
        val result = mutableListOf<Document>()
        resolver.query(query, columns, null, null, null)?.use { c ->
            while (c.moveToNext()) result += Document(Docs.buildDocumentUriUsingTree(p.grant.uri, c.getString(0)), c.getString(0), c.getString(1), c.getString(2), c.getLong(3), if(c.isNull(4)) null else c.getLong(4), if(c.isNull(5)) null else c.getLong(5))
        } ?: throw IllegalStateException("Provider did not enumerate the folder")
        return result
    }
    private fun child(p: TreePath, parent: Document, name: String): Document? {
        val found = children(p, parent).filter { it.name == name }
        check(found.size <= 1) { "Provider has ambiguous duplicate document names" }
        return found.firstOrNull()
    }
    private fun parent(p: TreePath, create: Boolean = false): Document {
        require(p.parts.isNotEmpty()) { "Expected a file below the authorized folder" }
        var current = root(p)
        for(name in p.parts.dropLast(1)) {
            check(current.mime == Docs.Document.MIME_TYPE_DIR)
            current = child(p, current, name) ?: if(create) create(p, current, name, true) else throw java.io.FileNotFoundException("Parent folder is missing")
        }
        check(current.mime == Docs.Document.MIME_TYPE_DIR) { "Parent is not a folder" }
        return current
    }
    private fun regular(doc: Document, write: Boolean) {
        check(doc.mime != Docs.Document.MIME_TYPE_DIR && doc.flags and Docs.Document.FLAG_VIRTUAL_DOCUMENT.toLong() == 0L) { "Expected a byte-addressable regular document" }
        if(write) {
            val needed = Docs.Document.FLAG_SUPPORTS_WRITE or Docs.Document.FLAG_SUPPORTS_RENAME or Docs.Document.FLAG_SUPPORTS_DELETE
            check(doc.flags and needed.toLong() == needed.toLong()) { "Provider must support reading, writing, rename and delete in the selected folder" }
        }
    }
    private fun create(p: TreePath, parent: Document, name: String, directory: Boolean = false): Document {
        check(parent.flags and Docs.Document.FLAG_DIR_SUPPORTS_CREATE.toLong() != 0L) { "Provider cannot create siblings in this folder" }
        check(child(p, parent, name) == null) { "Document already exists" }
        val uri = Docs.createDocument(resolver, parent.uri, if(directory) Docs.Document.MIME_TYPE_DIR else "application/octet-stream", name)
            ?: throw IllegalStateException("Provider failed to create document")
        val created = read(uri) ?: throw IllegalStateException("Created document is missing")
        if(created.name != name) {
            Docs.deleteDocument(resolver, uri)
            throw IllegalStateException("Provider changed the requested name; target already exists or exact names are unsupported")
        }
        try {
            check(child(p, parent, name)?.id == created.id) { "Provider did not create an unambiguous sibling" }
        } catch(error: Exception) {
            // Delete only the document this call created; never another same-name document.
            Docs.deleteDocument(resolver, uri)
            throw error
        }
        return created
    }
    private fun replyError(error: Exception, recovery: String? = null) = JSObject()
        .put("error", error.message ?: error.javaClass.simpleName)
        .put("recovery", recovery)
        .put("folderRequired", error is FolderAuthorizationException)

    @Command
    fun operate(invoke: Invoke) {
        val args = invoke.parseArgs(TreeArgs::class.java)
        worker.execute {
            try { invoke.resolve(operation(args)) }
            catch(error: Exception) { invoke.resolve(replyError(error)) }
        }
    }
    private fun operation(a: TreeArgs): JSObject {
        val p = path(a.path, a.write)
        if(a.op == "authorize") return JSObject()
        if(a.op == "resolve") {
            val parts = p.parts + segments(a.to ?: "")
            val target = TreePath(p.token, p.grant, parts)
            parent(target, a.write) // Only creates parent directories; never the formal file.
            return JSObject().put("location", location(p.token, parts))
        }
        if(a.op == "name") return JSObject().put("name", if(p.parts.isEmpty()) root(p).name else p.parts.last())
        if(a.op == "sibling") {
            val name = segments(a.to ?: "")
            require(name.size == 1 && p.parts.isNotEmpty())
            return JSObject().put("location", location(p.token, p.parts.dropLast(1) + name))
        }
        if(a.op == "walk") {
            val result = JSONArray()
            fun visit(doc: Document, parts: List<String>, depth: Int) {
                check(depth < 32) { "Folder depth exceeds 32" }
                for(entry in children(p, doc)) {
                    segments(entry.name).also { require(it.size == 1) }
                    val next = parts + entry.name
                    if(entry.mime == Docs.Document.MIME_TYPE_DIR) visit(entry, next, depth+1)
                    else { regular(entry, false); result.put(JSObject().put("location", location(p.token,p.parts+next)).put("relativePath",next.joinToString("/")).put("size",entry.size ?: throw IllegalStateException("Unknown source size"))) }
                }
            }
            visit(root(p), emptyList(), 0)
            return JSObject().put("entries", result)
        }
        val parent = parent(p)
        if(a.op == "canonical") return JSObject().put("identity", "android-saf:${p.grant.uri.authority}:${parent.id}/${p.parts.last()}")
        if(a.op == "create") {
            val doc = create(p,parent,p.parts.last())
            regular(doc,true)
            return JSObject().put("uri",doc.uri.toString())
        }
        val doc = child(p,parent,p.parts.last())
        if(a.op == "stat" && doc == null) return JSObject().put("absent",true)
        requireNotNull(doc) { "Document no longer exists" }
        regular(doc,a.write)
        if(a.op == "stat") return JSObject().put("identity","${p.grant.uri.authority}:${doc.id}").put("size",doc.size ?: throw IllegalStateException("Provider does not report source size")).put("modified",doc.modified)
        if(a.op == "open") return JSObject().put("uri",doc.uri.toString())
        if(a.op == "remove") { check(Docs.deleteDocument(resolver,doc.uri)); return JSObject() }
        if(a.op != "rename") throw IllegalArgumentException("Unknown document operation")
        val to = path(a.to ?: "",true)
        val toParent = parent(to)
        require(p.grant.uri.authority == to.grant.uri.authority && parent.id == toParent.id) { "Rename must stay in the same authorized folder" }
        val targetBefore = child(to,toParent,to.parts.last())
        if(!a.replace && targetBefore != null) return JSObject().put("conflict",true).put("error","Destination already exists")
        // SAF provides no atomic rename-if-absent contract. A preflight query
        // cannot protect a concurrently created .bak, so never move an old
        // destination through this path on an arbitrary document provider.
        if(!a.replace) throw UnsupportedOperationException("This document provider cannot guarantee a no-replace rename; the original and staged file are preserved")
        // Always attempt the provider rename first. An exception alone never
        // proves a collision, even when a same-name destination is present.
        val result = renameWithRecovery(doc.renameDocument(), to.parts.last(), targetBefore?.renameDocument(), object : DocumentRenameAccess {
            override fun rename(uri: String, name: String) = Docs.renameDocument(resolver, Uri.parse(uri), name)?.toString()
            override fun read(uri: String) = this@FileTreesPlugin.read(Uri.parse(uri))?.renameDocument()
            override fun original() = child(p, parent, doc.name)?.renameDocument()
            override fun target() = child(to, toParent, to.parts.last())?.renameDocument()
        })
        return JSObject().put("conflict", result.conflict).put("error", result.error).put("recovery", result.recovery)
    }
}
