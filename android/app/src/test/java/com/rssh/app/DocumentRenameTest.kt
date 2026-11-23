package com.rssh.app

import org.junit.Assert.*
import org.junit.Test

class DocumentRenameTest {
    @Test
    fun folderGrantsKeepReadSourcesSeparateFromWritableReceiversAndRejectExpiredTokens() {
        val readOnly = TreeGrant("content://provider/read-only-tree", false)
        val writable = TreeGrant("content://provider/writable-tree", true)
        val grants = mutableMapOf("source" to readOnly, "receiver" to writable)
        assertSame(readOnly, requireTreeGrant(grants, "source", false))
        assertSame(writable, requireTreeGrant(grants, "receiver", true))
        assertThrows(FolderAuthorizationException::class.java) { requireTreeGrant(grants, "source", true) }
        assertThrows(FolderAuthorizationException::class.java) { requireTreeGrant(grants, "unknown", false) }
        grants.remove("receiver")
        assertThrows(FolderAuthorizationException::class.java) { requireTreeGrant(grants, "receiver", true) }
        assertSame(readOnly, requireTreeGrant(grants, "source", false))
    }

    private class Provider : DocumentRenameAccess {
        val source = RenameDocument("content://provider/staged", "staged", ".owned.part", 20, 1)
        val oldTarget = RenameDocument("content://provider/original", "original", "target", 10, 1)
        val documents = mutableMapOf(source.uri to source, oldTarget.uri to oldTarget)
        var renames = 0
        var directReplacement = false
        var afterRename: (Int) -> Unit = {}
        var beforeRead: (Int) -> Unit = {}
        var beforeOriginal: () -> Unit = {}
        var beforeTarget: (Int) -> Unit = {}

        override fun rename(uri: String, name: String): String {
            renames++
            val original = documents.remove(uri)!!
            val actualName = if (renames == 1 && !directReplacement) "$name (1)" else name
            if (directReplacement) documents.remove(oldTarget.uri)
            val moved = original.copy(uri = "content://provider/renamed-$renames", id = "renamed-$renames", name = actualName)
            documents[moved.uri] = moved
            afterRename(renames) // Can fail after the provider committed the move.
            return moved.uri
        }

        override fun read(uri: String): RenameDocument? {
            beforeRead(renames)
            return documents[uri]
        }

        override fun original(): RenameDocument? {
            beforeOriginal()
            return documents.values.singleOrNull { it.name == source.name }
        }

        override fun target(): RenameDocument? {
            beforeTarget(renames)
            return documents.values.singleOrNull { it.name == oldTarget.name }
        }

        fun run() = renameWithRecovery(source, oldTarget.name, oldTarget, this)
    }

    @Test
    fun directReplacementSucceedsOnlyWithTheReturnedDocumentAtTheTarget() {
        val provider = Provider().apply { directReplacement = true }
        assertEquals(RenameResult(), provider.run())
        assertEquals(1, provider.renames)
        assertEquals(provider.source.size, provider.target()?.size)
    }

    @Test
    fun suffixAndConfirmedRestorationReportConflictWithoutMovingTheOldTarget() {
        val provider = Provider()
        val result = provider.run()
        assertTrue(result.conflict)
        assertNull(result.recovery)
        assertEquals(2, provider.renames)
        assertEquals(provider.oldTarget, provider.target())
        assertEquals(provider.source.size, provider.original()?.size)
    }

    @Test
    fun metadataFailureAfterFirstRenamePreservesTheReturnedUri() {
        val provider = Provider().apply { beforeRead = { throw IllegalStateException("provider disconnected") } }
        val result = provider.run()
        assertFalse(result.conflict)
        assertEquals("content://provider/renamed-1", result.recovery)
        assertTrue(provider.documents.containsKey(result.recovery))
        assertEquals(provider.oldTarget, provider.documents[provider.oldTarget.uri])
    }

    @Test
    fun sourceQueryFailureAfterSuffixKeepsTheLastConfirmedUri() {
        val provider = Provider().apply { beforeOriginal = { throw SecurityException("grant revoked") } }
        val result = provider.run()
        assertFalse(result.conflict)
        assertEquals("content://provider/renamed-1", result.recovery)
        assertEquals(1, provider.renames)
    }

    @Test
    fun restoredUriSurvivesMetadataAndTargetQueryFailures() {
        for (failMetadata in listOf(true, false)) {
            val provider = Provider().apply {
                if (failMetadata) beforeRead = { if (it == 2) throw IllegalStateException("metadata unavailable") }
                else beforeTarget = { if (it == 2) throw IllegalStateException("folder unavailable") }
            }
            val result = provider.run()
            assertFalse(result.conflict)
            assertEquals("content://provider/renamed-2", result.recovery)
            assertEquals(provider.source.name, provider.documents[result.recovery]?.name)
            assertEquals(provider.oldTarget, provider.documents[provider.oldTarget.uri])
        }
    }

    @Test
    fun targetChangedDuringRestorationReportsTheActualRestoredUri() {
        val provider = Provider().apply {
            afterRename = { if (it == 2) documents[oldTarget.uri] = oldTarget.copy(modified = 2) }
        }
        val result = provider.run()
        assertFalse(result.conflict)
        assertEquals("content://provider/renamed-2", result.recovery)
        assertEquals(provider.source.name, provider.documents[result.recovery]?.name)
    }

    @Test
    fun lostRestoreReplyKeepsLastKnownUriWithoutClaimingItStillExists() {
        val provider = Provider().apply {
            afterRename = { if (it == 2) throw IllegalStateException("restore reply lost") }
        }
        val result = provider.run()
        assertFalse(result.conflict)
        assertEquals("restore reply lost", result.error)
        assertEquals("content://provider/renamed-1", result.recovery)
        assertFalse(provider.documents.containsKey(result.recovery))
        assertEquals(provider.source.name, provider.documents["content://provider/renamed-2"]?.name)
        assertEquals(provider.oldTarget, provider.documents[provider.oldTarget.uri])
    }
}
