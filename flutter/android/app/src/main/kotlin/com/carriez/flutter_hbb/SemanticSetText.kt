package com.carriez.flutter_hbb

/**
 * `AccessibilityInputConnection.commitText` returns void, so the call itself
 * is not success. [readBack] is the following `getSurroundingText` snapshot
 * (`null` when that read fails). `ok` only when the snapshot is the whole
 * field and equals [requested]. A non-zero [offset] is a hidden prefix. An
 * after-cursor span that fills [window] can hide a tail.
 */
internal fun semanticCommitResult(
    requested: String,
    readBack: String?,
    offset: Int,
    selectionStart: Int,
    selectionEnd: Int,
    window: Int,
): String {
    if (readBack == null || offset != 0) {
        return "inject_failed"
    }
    if (selectionStart < 0 || selectionEnd < selectionStart || selectionEnd > readBack.length) {
        return "inject_failed"
    }
    if (readBack.length - selectionEnd >= window) {
        return "inject_failed"
    }
    return if (readBack == requested) "ok" else "inject_failed"
}
