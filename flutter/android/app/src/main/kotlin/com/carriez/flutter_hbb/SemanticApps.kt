package com.carriez.flutter_hbb

/**
 * Peer-result decisions for `open_app` and `list_apps`. No Android types, so
 * the unit test can run without an SDK.
 *
 * `open_app` codes are `ok`, `unsupported`, `no_launch_intent`, `start_threw`,
 * and `fg_timeout`. The three failures are not `inject_failed`.
 *
 * `list_apps` codes are `ok`, `unsupported`, and `inject_failed`. An empty
 * launcher set is `ok` with an empty array. Source rows are MAIN/LAUNCHER
 * only. Page size is 40 and the hard cap is 256.
 */

internal const val OPEN_OK = "ok"
internal const val OPEN_UNSUPPORTED = "unsupported"
internal const val OPEN_NO_LAUNCH_INTENT = "no_launch_intent"
internal const val OPEN_START_THREW = "start_threw"
internal const val OPEN_FG_TIMEOUT = "fg_timeout"

internal const val OPEN_APP_TIMEOUT_MS = 2_500L
internal const val OPEN_APP_POLL_MS = 50L

internal enum class OpenStart {
    STARTED,
    NO_LAUNCH_INTENT,
    START_THREW,
}

/** A failed start is never `ok`, even if that package is already in front. */
internal fun openAppPeerCode(start: OpenStart, sawPackage: Boolean): String = when (start) {
    OpenStart.NO_LAUNCH_INTENT -> OPEN_NO_LAUNCH_INTENT
    OpenStart.START_THREW -> OPEN_START_THREW
    OpenStart.STARTED -> if (sawPackage) OPEN_OK else OPEN_FG_TIMEOUT
}

/** Polls the foreground wait may take. An absurd timeout still stops at 64. */
internal fun openAppMaxPolls(
    timeoutMs: Long = OPEN_APP_TIMEOUT_MS,
    pollMs: Long = OPEN_APP_POLL_MS,
): Int {
    if (timeoutMs <= 0L || pollMs <= 0L) return 0
    val polls = (timeoutMs + pollMs - 1) / pollMs
    return if (polls > 64L) 64 else polls.toInt()
}

internal const val LIST_APPS_PAGE = 40
internal const val LIST_APPS_CAP = 256
internal const val LIST_APPS_LABEL_MAX = 512
internal const val LIST_APPS_NAME_MAX = 256

internal data class LauncherRow(
    val packageName: String,
    val label: String?,
    val activity: String?,
)

internal data class ListedApp(
    val packageName: String,
    val label: String?,
    val activity: String?,
)

internal class ListAppsPage(
    val code: String,
    val items: List<ListedApp>,
    val truncated: Boolean,
)

/** Empty is 0. Offsets past [LIST_APPS_CAP] are refused. */
internal fun listAppsOffset(arg: String): Int? {
    if (arg.isEmpty()) return 0
    if (arg.length > 3) return null
    if (arg.length > 1 && arg[0] == '0') return null
    for (ch in arg) {
        if (ch !in '0'..'9') return null
    }
    val n = arg.toIntOrNull() ?: return null
    if (n > LIST_APPS_CAP) return null
    return n
}

internal fun listAppsFromRows(arg: String, rows: List<LauncherRow>): ListAppsPage {
    val offset = listAppsOffset(arg) ?: return ListAppsPage(OPEN_UNSUPPORTED, emptyList(), false)
    return collectLauncherPage(offset, rows.iterator())
}

/**
 * Keeps at most [LIST_APPS_CAP] unique package/activity rows, then returns
 * one page. The iterator is not read after the cap is hit.
 */
internal fun collectLauncherPage(offset: Int, rows: Iterator<LauncherRow>): ListAppsPage {
    if (offset < 0 || offset > LIST_APPS_CAP) {
        return ListAppsPage(OPEN_UNSUPPORTED, emptyList(), false)
    }
    val kept = ArrayList<ListedApp>(LIST_APPS_CAP)
    val seen = HashSet<String>(LIST_APPS_CAP * 2)
    var truncated = false
    while (rows.hasNext()) {
        val row = rows.next()
        val pkg = row.packageName.trim()
        if (!packageNameOk(pkg)) continue
        val activity = normalizeActivity(row.activity)
        val key = pkg + "\u0000" + (activity ?: "")
        if (!seen.add(key)) continue
        if (kept.size == LIST_APPS_CAP) {
            truncated = true
            break
        }
        kept.add(ListedApp(pkg, normalizeLabel(row.label), activity))
    }
    kept.sortWith(compareBy({ it.packageName }, { it.activity ?: "" }))
    if (offset >= kept.size) {
        return ListAppsPage(OPEN_OK, emptyList(), truncated)
    }
    val end = minOf(offset + LIST_APPS_PAGE, kept.size)
    return ListAppsPage(OPEN_OK, kept.subList(offset, end).toList(), truncated)
}

/** `ok` is `ok\n` plus the JSON page. Any other code is the bare code. */
internal fun listAppsWire(page: ListAppsPage): String {
    if (page.code != OPEN_OK) return page.code
    return "ok\n" + listAppsJson(page)
}

internal fun packageNameOk(name: String): Boolean {
    if (name.isEmpty() || name.length > LIST_APPS_NAME_MAX) return false
    val parts = name.split('.')
    if (parts.size < 2) return false
    return parts.all { identSegment(it) }
}

internal fun activityNameOk(activity: String): Boolean {
    if (activity.length > LIST_APPS_NAME_MAX) return false
    if (activity.startsWith(".")) {
        val rest = activity.substring(1)
        if (rest.isEmpty()) return false
        return rest.split('.').all { identSegment(it) }
    }
    return packageNameOk(activity)
}

internal fun normalizeLabel(raw: String?): String? {
    if (raw == null) return null
    val cleaned = buildString(raw.length) {
        for (ch in raw) {
            append(if (ch.isISOControl()) ' ' else ch)
        }
    }.trim()
    if (cleaned.isEmpty()) return null
    if (cleaned.toByteArray(Charsets.UTF_8).size > LIST_APPS_LABEL_MAX) return null
    return cleaned
}

internal fun normalizeActivity(raw: String?): String? {
    if (raw == null) return null
    val name = raw.trim()
    if (name.isEmpty() || !activityNameOk(name)) return null
    return name
}

private fun identSegment(seg: String): Boolean {
    if (seg.isEmpty()) return false
    val first = seg[0]
    if (first !in 'A'..'Z' && first !in 'a'..'z') return false
    for (i in 1 until seg.length) {
        val c = seg[i]
        if (c !in 'A'..'Z' && c !in 'a'..'z' && c !in '0'..'9' && c != '_') return false
    }
    return true
}

internal fun listAppsJson(page: ListAppsPage): String {
    val sb = StringBuilder()
    sb.append("{\"items\":[")
    page.items.forEachIndexed { i, app ->
        if (i > 0) sb.append(',')
        sb.append("{\"package\":")
        sb.append(jsonString(app.packageName))
        sb.append(",\"label\":")
        if (app.label == null) sb.append("null") else sb.append(jsonString(app.label))
        sb.append(",\"activity\":")
        if (app.activity == null) sb.append("null") else sb.append(jsonString(app.activity))
        sb.append('}')
    }
    sb.append("],\"truncated\":")
    sb.append(if (page.truncated) "true" else "false")
    sb.append('}')
    return sb.toString()
}

private fun jsonString(value: String): String {
    val sb = StringBuilder(value.length + 2)
    sb.append('"')
    for (ch in value) {
        when (ch) {
            '\\' -> sb.append("\\\\")
            '"' -> sb.append("\\\"")
            '\n' -> sb.append("\\n")
            '\r' -> sb.append("\\r")
            '\t' -> sb.append("\\t")
            else -> {
                if (ch.code < 0x20) {
                    sb.append("\\u%04x".format(ch.code))
                } else {
                    sb.append(ch)
                }
            }
        }
    }
    sb.append('"')
    return sb.toString()
}
