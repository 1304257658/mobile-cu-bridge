package com.carriez.flutter_hbb

/**
 * `open_app` peer-result decisions. No Android types, so the unit test can
 * run without an SDK.
 */

internal const val OPEN_OK = "ok"
internal const val OPEN_UNSUPPORTED = "unsupported"
internal const val OPEN_INJECT_FAILED = "inject_failed"
internal const val OPEN_NOT_INSTALLED = "inject_failed: not_installed"
internal const val OPEN_NO_PERMISSION = "inject_failed: no_permission"
internal const val OPEN_TIMEOUT = "inject_failed: timeout"

internal const val OPEN_APP_TIMEOUT_MS = 2_500L
internal const val OPEN_APP_POLL_MS = 50L

internal enum class OpenStart {
    STARTED,
    NOT_INSTALLED,
    NO_PERMISSION,
    FAILED,
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
