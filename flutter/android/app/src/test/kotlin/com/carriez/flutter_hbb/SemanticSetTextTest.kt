package com.carriez.flutter_hbb

private const val WINDOW = 16 * 1024

fun main() {
    expect("missing read-back", semanticCommitResult("hi", null, 0, 0, 0, WINDOW), "inject_failed")
    expect("different field", semanticCommitResult("hi", "hello", 0, 5, 5, WINDOW), "inject_failed")
    expect("hidden prefix", semanticCommitResult("hi", "hi", 3, 2, 2, WINDOW), "inject_failed")
    expect("read-back matches", semanticCommitResult("hi", "hi", 0, 2, 2, WINDOW), "ok")
    expect("cleared field", semanticCommitResult("", "", 0, 0, 0, WINDOW), "ok")
    val filled = "abcdefgh"
    expect(
        "after-window full",
        semanticCommitResult(filled, filled, 0, 0, 0, filled.length),
        "inject_failed",
    )
}

private fun expect(label: String, actual: String, expected: String) {
    check(actual == expected) { "$label: expected $expected, got $actual" }
}
