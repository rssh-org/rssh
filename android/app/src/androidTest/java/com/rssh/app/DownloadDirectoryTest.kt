package com.rssh.app

import android.Manifest
import android.app.Activity
import android.content.ContentResolver
import android.content.ContextWrapper
import android.content.pm.PackageManager
import android.content.pm.ProviderInfo
import android.database.MatrixCursor
import android.net.Uri
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import android.provider.DocumentsProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.SdkSuppress
import androidx.test.platform.app.InstrumentationRegistry
import app.tauri.plugin.Invoke
import com.fasterxml.jackson.databind.ObjectMapper
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
@SdkSuppress(minSdkVersion = 29) // ContentResolver.wrap
class DownloadDirectoryTest {
    private val name = ".rssh-test.part"
    private val authority = "com.rssh.app.test.documents"
    private val tree = DocumentsContract.buildTreeDocumentUri(authority, "root")
    private lateinit var directory: LocalDirectory
    private lateinit var granted: ContextWrapper
    private lateinit var provider: TestDocuments
    private lateinit var backing: File

    @Before fun setup() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        backing = File.createTempFile("download-directory-", ".bin", context.cacheDir)
        provider = TestDocuments(backing)
        provider.attachInfo(context, ProviderInfo().apply {
            authority = this@DownloadDirectoryTest.authority
            exported = true
            grantUriPermissions = true
            readPermission = Manifest.permission.MANAGE_DOCUMENTS
            writePermission = Manifest.permission.MANAGE_DOCUMENTS
        })
        val resolver = ContentResolver.wrap(provider)
        granted = object : ContextWrapper(context) {
            override fun getContentResolver() = resolver
            override fun checkUriPermission(uri: Uri, pid: Int, uid: Int, flags: Int) =
                if (uri == tree) PackageManager.PERMISSION_GRANTED else PackageManager.PERMISSION_DENIED
        }
        directory = LocalDirectory(granted, tree)
    }

    @After fun cleanup() { backing.delete() }

    private fun failed(create: Boolean = true, accept: (ParcelFileDescriptor) -> Unit = { fail("Unexpected writer") }): Exception {
        try {
            directory.openWriter(name, create, accept)
        } catch (e: Exception) {
            return e
        }
        throw AssertionError("Expected writer preparation to fail")
    }

    @Test fun missingCapabilitiesRemoveOnlyCreatedUri() {
        provider.flags = Document.FLAG_SUPPORTS_WRITE
        failed()
        assertEquals(listOf("created"), provider.deleted)
        assertEquals(mapOf("other" to "other.part"), provider.names)
    }

    @Test fun autoNumberedCreateKeepsRacingDocumentAtRequestedName() {
        provider.numbered = true
        failed()
        assertEquals(listOf("created"), provider.deleted)
        assertEquals(name, provider.names["racing"])
        assertEquals("other.part", provider.names["other"])
    }

    @Test fun existingPartialNeverAcquiredByCreateOrReopenFailure() {
        provider.names["existing"] = name
        failed()
        provider.openFailure = true
        failed(create = false)
        assertEquals(emptyList<String>(), provider.deleted)
        assertEquals(name, provider.names["existing"])
        assertFalse(provider.names.containsKey("created"))
    }

    @Test fun rejectedOpenAndNonSeekableDescriptorReportUnidentifiedPartial() {
        provider.openFailure = true
        assertTrue(failed().message.orEmpty().contains("Test folder/$name"))
        assertEquals(name, provider.names.remove("created"))
        provider.openFailure = false
        provider.pipe = true
        assertTrue(failed().message.orEmpty().contains("Test folder/$name"))
        assertEquals(name, provider.names["created"])
        assertEquals(emptyList<String>(), provider.deleted)
    }

    @Test fun failedDescriptorHandoffRollsBackAndSuccessTransfersOwnership() {
        var attempted = false
        failed { original ->
            attempted = true
            val duplicate = original.dup()
            duplicate.close()
            duplicate.dup() // fails before Rust can acquire its descriptor
        }
        assertTrue(attempted)
        assertEquals(listOf("created"), provider.deleted)
        directory.openWriter(name, true) { original -> original.use { it.dup().close() } }
        assertEquals(name, provider.names["created"])
        assertEquals(listOf("created"), provider.deleted)
    }

    @Test fun failedCleanupReportsActualNameOrReturnedUri() {
        provider.numbered = true
        provider.refuseDelete = true
        assertTrue(failed().message.orEmpty().contains("Test folder/.rssh-test (1).part"))
        assertEquals(name, provider.names["racing"])
        provider.hideCreatedMetadata = true
        val created = DocumentsContract.buildDocumentUriUsingTree(tree, "created")
        // Start the second case with a clean folder and deny its metadata query.
        provider.names.remove("racing")
        provider.names.remove("created")
        assertTrue(failed().message.orEmpty().contains(created.toString()))
        assertEquals(listOf("created"), provider.deleted)
    }

    @Test fun failedHandoffKeepsReplacementAtPathBasedUri() {
        val displaced = File.createTempFile("download-directory-moved-", ".part", backing.parentFile)
        try {
            val error = failed {
                assertTrue(backing.renameTo(displaced))
                backing.writeText("unrelated replacement")
                throw IOException("Descriptor handoff failed")
            }
            assertTrue(error.message.orEmpty().contains("Test folder/$name"))
            assertEquals(emptyList<String>(), provider.deleted)
            assertEquals("unrelated replacement", backing.readText())
            assertEquals(name, provider.names["created"])
        } finally {
            displaced.delete()
        }
    }

    @Test fun pluginRejectsWithActualLocationWhenHandoffAndRollbackFail() {
        provider.refuseDelete = true
        lateinit var plugin: DownloadDirectoryPlugin
        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            plugin = DownloadDirectoryPlugin(object : Activity() {
                override fun getApplicationContext() = granted
            })
        }
        val args = JSONObject().put("op", "create").put("name", name).put("tree", tree.toString())
        val rejection = CompletableFuture<String>()
        val invoke = Invoke(1, "fileOperation", 2, 3, { callback, data ->
            if (callback == 2L) throw IOException("Descriptor handoff failed")
            rejection.complete(data)
        }, args.toString(), ObjectMapper())
        plugin.fileOperation(invoke)
        val message = JSONObject(rejection.get(10, TimeUnit.SECONDS)).getString("message")
        assertTrue(message, message.contains("Test folder/$name"))
        assertEquals(listOf("created"), provider.deleted)
        assertEquals("other.part", provider.names["other"])
    }

    private class TestDocuments(private val backing: File) : DocumentsProvider() {
        val names = mutableMapOf("other" to "other.part")
        val deleted = mutableListOf<String>()
        var flags = Document.FLAG_SUPPORTS_WRITE or Document.FLAG_SUPPORTS_DELETE or Document.FLAG_SUPPORTS_RENAME
        var numbered = false
        var openFailure = false
        var pipe = false
        var refuseDelete = false
        var hideCreatedMetadata = false

        override fun onCreate() = true
        override fun isChildDocument(parentDocumentId: String, documentId: String) =
            parentDocumentId == "root" && names.containsKey(documentId)
        override fun queryRoots(projection: Array<out String>?) = MatrixCursor(projection ?: emptyArray())

        private fun rows(ids: Collection<String>, projection: Array<out String>?): MatrixCursor {
            return MatrixCursor(projection ?: emptyArray()).apply {
                for (id in ids) {
                    val row = newRow()
                    for (column in columnNames) row.add(when (column) {
                        Document.COLUMN_DOCUMENT_ID -> id
                        Document.COLUMN_DISPLAY_NAME -> if (id == "root") "Test folder" else names.getValue(id)
                        Document.COLUMN_MIME_TYPE -> if (id == "root") Document.MIME_TYPE_DIR else "application/octet-stream"
                        Document.COLUMN_FLAGS -> if (id == "root") Document.FLAG_DIR_SUPPORTS_CREATE else flags
                        else -> null
                    })
                }
            }
        }
        override fun queryDocument(documentId: String, projection: Array<out String>?) =
            rows(if (hideCreatedMetadata && documentId == "created") emptyList() else listOf(documentId), projection)
        override fun queryChildDocuments(parentDocumentId: String, projection: Array<out String>?, sortOrder: String?) =
            rows(names.keys, projection)
        override fun createDocument(parentDocumentId: String, mimeType: String, displayName: String): String {
            if (numbered) names["racing"] = displayName
            names["created"] = if (numbered) ".rssh-test (1).part" else displayName
            backing.createNewFile()
            return "created"
        }
        override fun deleteDocument(documentId: String) {
            deleted.add(documentId)
            if (refuseDelete) throw FileNotFoundException("Delete rejected")
            names.remove(documentId)
            if (documentId == "created") backing.delete()
        }
        override fun openDocument(documentId: String, mode: String, signal: CancellationSignal?): ParcelFileDescriptor {
            if (openFailure) throw FileNotFoundException("Open rejected")
            if (pipe) {
                val ends = ParcelFileDescriptor.createPipe()
                ends[0].close()
                return ends[1]
            }
            return ParcelFileDescriptor.open(backing, ParcelFileDescriptor.MODE_READ_WRITE)
        }
    }
}
