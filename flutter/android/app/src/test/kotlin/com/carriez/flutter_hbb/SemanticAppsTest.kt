package com.carriez.flutter_hbb

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
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

    @Test
    fun listOffset() {
        assertEquals(0, listAppsOffset(""))
        assertEquals(0, listAppsOffset("0"))
        assertEquals(40, listAppsOffset("40"))
        assertEquals(256, listAppsOffset("256"))
        assertNull(listAppsOffset("257"))
        assertNull(listAppsOffset("00"))
        assertNull(listAppsOffset("-1"))
        assertNull(listAppsOffset("1a"))
    }

    @Test
    fun pageIsDisplayNameAndPackage() {
        val reply = listAppsPage(
            listOf(
                LaunchableApp("Zed", "com.z"),
                LaunchableApp("Aye", "com.a"),
            ),
            0,
        )
        assertEquals(OPEN_OK, reply.code)
        assertEquals(
            "v1\t0\t2\t2\t0\nAye\tcom.a\nZed\tcom.z\n",
            reply.detail,
        )
        assertEquals("ok\n" + reply.detail, reply.wire())
    }

    @Test
    fun secondPageOmitsTheFirst() {
        val apps = listOf(
            LaunchableApp("Aye", "com.a"),
            LaunchableApp("Bee", "com.b"),
        )
        val reply = listAppsPage(apps, 1, pageSize = 1)
        assertEquals(OPEN_OK, reply.code)
        assertEquals("v1\t1\t1\t2\t0\nBee\tcom.b\n", reply.detail)
    }

    @Test
    fun offsetPastTheEndIsAnEmptyPage() {
        val reply = listAppsPage(listOf(LaunchableApp("Aye", "com.a")), 1)
        assertEquals(OPEN_OK, reply.code)
        assertEquals("v1\t1\t0\t1\t0\n", reply.detail)
    }

    @Test
    fun duplicatePackageIsOneRow() {
        val reply = listAppsPage(
            listOf(
                LaunchableApp("Bee", "com.a"),
                LaunchableApp("Aye", "com.a"),
            ),
            0,
        )
        assertEquals("v1\t0\t1\t1\t0\nAye\tcom.a\n", reply.detail)
    }

    @Test
    fun dirtyNameIsSignaledOrSanitized() {
        val reply = listAppsPage(
            listOf(
                LaunchableApp("A\tB\nC", "com.ok"),
                LaunchableApp("Bad", "has space"),
                LaunchableApp("", "com.empty.name"),
            ),
            0,
        )
        assertEquals(OPEN_OK, reply.code)
        assertEquals(
            "v1\t0\t2\t2\t1\n?\tcom.empty.name\nA B C\tcom.ok\n",
            reply.detail,
        )
    }

    @Test
    fun overCapDoesNotReturnAPage() {
        val apps = ArrayList<LaunchableApp>(LIST_APPS_HARD_CAP + 1)
        for (i in 0..LIST_APPS_HARD_CAP) {
            apps.add(LaunchableApp("Name$i", "com.example.app$i"))
        }
        val reply = listAppsPage(apps, 0)
        assertEquals(OPEN_CAP, reply.code)
        assertEquals("", reply.detail)
        assertEquals(OPEN_CAP, reply.wire())
    }

    @Test
    fun oversizedPageTextIsCapNotATruncatedPage() {
        val reply = listAppsPage(
            listOf(LaunchableApp("Settings", "com.android.settings")),
            0,
            detailMax = 4,
        )
        assertEquals(OPEN_CAP, reply.code)
        assertEquals("", reply.detail)
    }

    @Test
    fun maxPageFitsTheProductionCap() {
        val name = "N".repeat(LIST_APPS_NAME_MAX)
        val apps = ArrayList<LaunchableApp>(LIST_APPS_PAGE)
        for (i in 0 until LIST_APPS_PAGE) {
            apps.add(LaunchableApp(name, maxPackage(i)))
        }
        val reply = listAppsPage(apps, 0)
        assertEquals(OPEN_OK, reply.code)
        assertTrue(reply.detail.length <= LIST_APPS_DETAIL_MAX)
        assertEquals(LIST_APPS_PAGE, reply.detail.trimEnd().lines().drop(1).size)
    }

    private fun maxPackage(i: Int): String {
        val suffix = i.toString()
        val pad = LIST_APPS_PACKAGE_MAX - 2 - suffix.length
        return "a." + "b".repeat(pad) + suffix
    }
}
