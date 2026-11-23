package com.rssh.app

import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Test

class DownloadDirectoryTest {
    @Test
    fun descriptorFailureRemovesTheCreatedDocument() {
        var discarded = false
        val refused = IOException("Provider returned a pipe")
        val error = assertThrows(IOException::class.java) {
            prepareCreatedDocument("folder/staging.part", prepare = { throw refused }, discard = { discarded = true })
        }
        assertSame(refused, error)
        assertEquals(true, discarded)
    }

    @Test
    fun failedCleanupReportsTheNewDocumentsRecoveryLocation() {
        val path = "folder/staging.part"
        val error = assertThrows(DirectoryError::class.java) {
            prepareCreatedDocument(path, prepare = { throw IOException("Cannot seek") }, discard = { error("Grant revoked") })
        }
        assertEquals("sftp_staging_retained", error.code)
        assertEquals(path, error.path)
    }
}
