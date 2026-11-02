package com.carriez.flutter_hbb

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SemanticAppsTest {
    @Test
    fun failedStartIsNotForegroundSuccess() {
        assertEquals("no_launch_intent", openAppPeerCode(OpenStart.NO_LAUNCH_INTENT, true))
        assertEquals("start_threw", openAppPeerCode(OpenStart.START_THREW, true))
        assertFalse(openAppPeerCode(OpenStart.NO_LAUNCH_INTENT, true).startsWith("inject_failed"))
        assertFalse(openAppPeerCode(OpenStart.START_THREW, false).startsWith("inject_failed"))
    }

    @Test
    fun startedWithoutForegroundIsTimeout() {
        assertEquals("fg_timeout", openAppPeerCode(OpenStart.STARTED, false))
        assertEquals(OPEN_OK, openAppPeerCode(OpenStart.STARTED, true))
        assertFalse(OPEN_FG_TIMEOUT.startsWith("inject_failed"))
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
