package com.carriez.flutter_hbb

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SemanticAppsTest {
    @Test
    fun failedStartIsNotForegroundSuccess() {
        assertEquals(OPEN_NOT_INSTALLED, openAppPeerCode(OpenStart.NOT_INSTALLED, true))
        assertEquals(OPEN_NO_PERMISSION, openAppPeerCode(OpenStart.NO_PERMISSION, true))
        assertEquals(OPEN_INJECT_FAILED, openAppPeerCode(OpenStart.FAILED, false))
    }

    @Test
    fun startedWithoutForegroundIsTimeout() {
        assertEquals(OPEN_TIMEOUT, openAppPeerCode(OpenStart.STARTED, false))
        assertEquals(OPEN_OK, openAppPeerCode(OpenStart.STARTED, true))
    }

    @Test
    fun foregroundWaitIsCapped() {
        assertEquals(50, openAppMaxPolls())
        assertEquals(64, openAppMaxPolls(timeoutMs = 60L * 60L * 1000L))
        assertEquals(0, openAppMaxPolls(timeoutMs = 0L))
        assertTrue(OPEN_APP_TIMEOUT_MS <= 2_500L)
        assertTrue(openAppMaxPolls() * OPEN_APP_POLL_MS <= OPEN_APP_TIMEOUT_MS)
    }
}
