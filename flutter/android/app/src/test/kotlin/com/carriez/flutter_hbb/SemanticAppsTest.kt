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

    @Test
    fun emptyLauncherSetIsOkAndBadOffsetIsUnsupported() {
        val page = listAppsFromRows("", emptyList())
        assertEquals(OPEN_OK, page.code)
        assertTrue(page.items.isEmpty())
        assertFalse(page.truncated)
        val wire = listAppsWire(page)
        assertTrue(wire.startsWith("ok\n"))
        assertTrue(wire.contains("\"items\":[]"))
        assertTrue(wire.contains("\"truncated\":false"))
        assertEquals("unsupported", listAppsWire(ListAppsPage(OPEN_UNSUPPORTED, emptyList(), false)))
        assertEquals("inject_failed", listAppsWire(ListAppsPage("inject_failed", emptyList(), false)))
        assertEquals(null, listAppsOffset("257"))
        assertEquals(null, listAppsOffset("-1"))
        assertEquals(null, listAppsOffset("00"))
        assertEquals(0, listAppsOffset(""))
        assertEquals(256, listAppsOffset("256"))
        assertEquals(OPEN_UNSUPPORTED, listAppsFromRows("257", emptyList()).code)
        assertFalse(listAppsFromRows("257", emptyList()).code.startsWith("inject_failed"))
    }

    @Test
    fun pagesFortyAndStopsAtTheCap() {
        val rows = (0 until 41).map {
            LauncherRow("com.example.p%02d".format(it), "Name", ".Main")
        }
        val first = listAppsFromRows("0", rows)
        assertEquals(OPEN_OK, first.code)
        assertEquals(LIST_APPS_PAGE, first.items.size)
        assertEquals("com.example.p00", first.items[0].packageName)
        assertEquals(".Main", first.items[0].activity)
        assertFalse(first.truncated)
        val second = listAppsFromRows("40", rows)
        assertEquals(1, second.items.size)
        assertEquals("com.example.p40", second.items[0].packageName)
        assertFalse(second.truncated)

        var walked = 0
        val capped = collectLauncherPage(240, object : Iterator<LauncherRow> {
            override fun hasNext() = true
            override fun next(): LauncherRow {
                check(walked <= LIST_APPS_CAP)
                val row = LauncherRow("com.example.n$walked", null, ".Main")
                walked += 1
                return row
            }
        })
        assertEquals(16, capped.items.size)
        assertTrue(capped.truncated)
        assertEquals(OPEN_OK, capped.code)
        assertEquals(LIST_APPS_CAP + 1, walked)

        var exact = 0
        val full = collectLauncherPage(256, object : Iterator<LauncherRow> {
            override fun hasNext() = exact < LIST_APPS_CAP
            override fun next(): LauncherRow {
                val row = LauncherRow("com.example.n$exact", "L", null)
                exact += 1
                return row
            }
        })
        assertTrue(full.items.isEmpty())
        assertFalse(full.truncated)
        assertEquals(LIST_APPS_CAP, exact)
    }

    @Test
    fun packageIsRequiredAndBlankLabelIsNull() {
        val page = listAppsFromRows(
            "",
            listOf(
                LauncherRow("", "skip", ".Main"),
                LauncherRow("com.example.app", "  \n", ".Main"),
                LauncherRow("com.example.app", "Again", ".Main"),
                LauncherRow("com.example.app", "A \"B\"", "com.example.app.Main"),
                LauncherRow("settings", "nope", null),
                LauncherRow("com.example.other", null, "not a name"),
            ),
        )
        assertEquals(OPEN_OK, page.code)
        assertEquals(3, page.items.size)
        assertEquals("com.example.app", page.items[0].packageName)
        assertEquals(null, page.items[0].label)
        assertEquals(".Main", page.items[0].activity)
        assertEquals("A \"B\"", page.items[1].label)
        assertEquals("com.example.app.Main", page.items[1].activity)
        assertEquals("com.example.other", page.items[2].packageName)
        assertEquals(null, page.items[2].label)
        assertEquals(null, page.items[2].activity)
        assertFalse(page.truncated)
        val wire = listAppsWire(page)
        assertTrue(wire.contains("\\\"B\\\""))
        assertFalse(wire.contains("settings"))
    }
}
