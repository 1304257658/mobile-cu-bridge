package com.carriez.flutter_hbb

/**
 * `open_app` peer-result decisions. No Android types, so the unit test can
 * run without an SDK.
 *
 * Codes are `ok`, `unsupported`, `no_launch_intent`, `start_threw`, and
 * `fg_timeout`. The three failures are not `inject_failed`.
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
