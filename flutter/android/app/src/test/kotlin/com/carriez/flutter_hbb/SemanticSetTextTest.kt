package com.carriez.flutter_hbb

import org.junit.Assert.assertEquals
import org.junit.Test

private const val WINDOW = 16 * 1024

class SemanticSetTextTest {
    @Test
    fun missingReadBack() {
        assertEquals("inject_failed", semanticCommitResult("hi", null, 0, 0, 0, WINDOW))
    }

    @Test
    fun textMismatch() {
        assertEquals("inject_failed", semanticCommitResult("hi", "hello", 0, 5, 5, WINDOW))
    }

    @Test
    fun hiddenPrefix() {
        assertEquals("inject_failed", semanticCommitResult("hi", "hi", 3, 2, 2, WINDOW))
    }

    @Test
    fun exactMatch() {
        assertEquals("ok", semanticCommitResult("hi", "hi", 0, 2, 2, WINDOW))
    }

    @Test
    fun emptyClear() {
        assertEquals("ok", semanticCommitResult("", "", 0, 0, 0, WINDOW))
    }

    @Test
    fun saturatedAfterWindow() {
        val filled = "a".repeat(WINDOW)
        assertEquals(
            "inject_failed",
            semanticCommitResult(filled, filled, 0, 0, 0, WINDOW),
        )
    }
}
