package com.carriez.flutter_hbb

/**
 * Decisions for `open_app` and `list_apps`. No Android types, so the unit
 * test can run without an SDK.
 *
 * `list_apps` detail, one record per line:
 * `v1\t<offset>\t<pageCount>\t<total>\t<skipped>`
 * then `<display>\t<package>`.
 * `total` is unique valid packages. `skipped` is rows dropped for a package
 * token that cannot sit in one field. A second launcher activity for the
 * same package is one row.
 */

internal const val OPEN_OK = "ok"
internal const val OPEN_UNSUPPORTED = "unsupported"
internal const val OPEN_INJECT_FAILED = "inject_failed"
internal const val OPEN_NOT_INSTALLED = "inject_failed: not_installed"
internal const val OPEN_NO_PERMISSION = "inject_failed: no_permission"
internal const val OPEN_TIMEOUT = "inject_failed: timeout"
internal const val OPEN_CAP = "inject_failed: cap"

internal const val OPEN_APP_TIMEOUT_MS = 2_500L
internal const val OPEN_APP_POLL_MS = 50L

internal const val LIST_APPS_PAGE = 40
internal const val LIST_APPS_HARD_CAP = 256
internal const val LIST_APPS_NAME_MAX = 80
internal const val LIST_APPS_PACKAGE_MAX = 256

/** One page of max-length rows fits. A larger body is refused. Same cap as Rust. */
internal const val LIST_APPS_DETAIL_MAX = 16 * 1024

internal enum class OpenStart {
    STARTED,
    NOT_INSTALLED,
    NO_PERMISSION,
    FAILED,
}

internal data class LaunchableApp(val name: String, val packageName: String)

internal class ListAppsReply(val code: String, val detail: String) {
    fun wire(): String {
        if (code != OPEN_OK || detail.isEmpty()) return code
        return "$code\n$detail"
    }
}

/** A failed start is never `ok`, even if that package is already in front. */
internal fun openAppPeerCode(start: OpenStart, sawPackage: Boolean): String = when (start) {
    OpenStart.NOT_INSTALLED -> OPEN_NOT_INSTALLED
    OpenStart.NO_PERMISSION -> OPEN_NO_PERMISSION
    OpenStart.FAILED -> OPEN_INJECT_FAILED
    OpenStart.STARTED -> if (sawPackage) OPEN_OK else OPEN_TIMEOUT
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

/** Empty is page 0. Digits only, no leading zero, and not past the hard cap. */
internal fun listAppsOffset(arg: String, hardCap: Int = LIST_APPS_HARD_CAP): Int? {
    if (arg.isEmpty()) return 0
    if (arg.length > 3 || arg.length > 1 && arg[0] == '0') return null
    for (ch in arg) {
        if (ch !in '0'..'9') return null
    }
    val n = arg.toIntOrNull() ?: return null
    if (n > hardCap) return null
    return n
}

internal fun launchablePackageOk(name: String): Boolean {
    if (name.isEmpty() || name.length > LIST_APPS_PACKAGE_MAX) return false
    for (ch in name) {
        if (ch == '\t' || ch == '\n' || ch == '\r' || ch == ' ') return false
    }
    return true
}

internal fun sanitizeDisplayName(raw: String, max: Int = LIST_APPS_NAME_MAX): String {
    if (max <= 0) return "?"
    val out = StringBuilder(minOf(raw.length, max))
    for (ch in raw) {
        if (out.length >= max) break
        out.append(if (ch == '\t' || ch == '\n' || ch == '\r') ' ' else ch)
    }
    return if (out.isEmpty()) "?" else out.toString()
}

/**
 * One page, or [OPEN_CAP] with an empty detail when [apps] is past [hardCap]
 * or the page text would pass [detailMax]. The over-cap path does not encode
 * the rows.
 */
internal fun listAppsPage(
    apps: List<LaunchableApp>,
    offset: Int,
    pageSize: Int = LIST_APPS_PAGE,
    hardCap: Int = LIST_APPS_HARD_CAP,
    detailMax: Int = LIST_APPS_DETAIL_MAX,
): ListAppsReply {
    if (offset < 0 || pageSize <= 0 || pageSize > hardCap || hardCap <= 0) {
        return ListAppsReply(OPEN_UNSUPPORTED, "")
    }
    if (apps.size > hardCap) {
        return ListAppsReply(OPEN_CAP, "")
    }
    val valid = ArrayList<LaunchableApp>(apps.size)
    var skipped = 0
    for (app in apps) {
        if (!launchablePackageOk(app.packageName)) {
            skipped++
            continue
        }
        valid.add(LaunchableApp(sanitizeDisplayName(app.name), app.packageName))
    }
    val unique = deduped(valid)
    val from = offset.coerceAtMost(unique.size)
    val to = (from + pageSize).coerceAtMost(unique.size)
    val page = unique.subList(from, to)
    val text = buildString {
        append("v1\t")
        append(offset)
        append('\t')
        append(page.size)
        append('\t')
        append(unique.size)
        append('\t')
        append(skipped)
        append('\n')
        for (app in page) {
            append(app.name)
            append('\t')
            append(app.packageName)
            append('\n')
        }
    }
    if (text.length > detailMax) {
        return ListAppsReply(OPEN_CAP, "")
    }
    return ListAppsReply(OPEN_OK, text)
}

private fun deduped(apps: List<LaunchableApp>): List<LaunchableApp> {
    val sorted = apps.sortedWith(compareBy({ it.packageName }, { it.name }))
    val out = ArrayList<LaunchableApp>(sorted.size)
    var previous = ""
    for (app in sorted) {
        if (app.packageName == previous) continue
        previous = app.packageName
        out.add(app)
    }
    return out
}
