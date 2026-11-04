package com.carriez.flutter_hbb

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PeerApkUpdateTest {
    private val hex = "ab".repeat(32)
    private val other = "cd".repeat(32)

    @Test
    fun sameSignatureIsTheOnlyInstallPlan() {
        assertEquals(UPDATE_OK, installPlan(hex, hex.uppercase(), "com.carriez.flutter_hbb", "com.carriez.flutter_hbb"))
        assertEquals(UPDATE_SIGNATURE, installPlan(hex, other, "com.carriez.flutter_hbb", "com.carriez.flutter_hbb"))
        assertEquals(UPDATE_SIGNATURE, installPlan("", hex, "com.carriez.flutter_hbb", "com.carriez.flutter_hbb"))
        assertEquals(UPDATE_SIGNATURE, installPlan(hex, hex, "com.carriez.flutter_hbb", "com.other.app"))
        assertEquals(UPDATE_SIGNATURE, installPlan(hex, hex, "", ""))
        val wire = certWire(UPDATE_OK, hex, hex.uppercase())
        assertTrue(wire.startsWith("ok\n"))
        assertFalse(wire.contains("uninstall"))
        assertEquals(UPDATE_SIGNATURE, certWire(UPDATE_SIGNATURE, hex, other))
        assertEquals(UPDATE_INSTALL, apkFileCode("other.apk", 10, false, true))
        assertEquals(UPDATE_INSTALL, apkFileCode(PEER_APK_NAME, PEER_APK_MAX_BYTES + 1, false, true))
        assertEquals(UPDATE_INSTALL, apkFileCode(PEER_APK_NAME, 10, true, true))
        assertNull(apkFileCode(PEER_APK_NAME, PEER_APK_MAX_BYTES, false, true))
        assertTrue(PEER_APK_CHUNK == 8192)
        assertTrue(PEER_APK_INSTALL_TIMEOUT_MS == 40_000L)
        assertFalse(UPDATE_OK == "uninstall")
    }

    @Test
    fun idChangeIsNotOk() {
        assertEquals(UPDATE_OK, idPreserved("123456789", "enc-v1", "enc-v1", "", ""))
        assertEquals(UPDATE_ID_CHANGED, idPreserved("123456789", "enc-v1", "", "123456789", "123456789"))
        assertEquals(UPDATE_ID_CHANGED, idPreserved("123456789", "enc-v1", "enc-v2", "", ""))
        assertEquals(UPDATE_ID_CHANGED, idPreserved("", "enc-v1", "enc-v1", "", ""))
        assertEquals(UPDATE_OK, idPreserved("123456789", "", "", "123456789", "123456789"))
        assertEquals(UPDATE_ID_CHANGED, idPreserved("123456789", "", "", "123456789", "999"))
        for (code in listOf(UPDATE_NETWORK, UPDATE_CHECKSUM, UPDATE_SIGNATURE, UPDATE_INSTALL, UPDATE_ID_CHANGED)) {
            assertFalse(code == UPDATE_OK)
            assertFalse(code.startsWith("inject_failed"))
            assertTrue(code.length <= 32)
        }
    }
}
