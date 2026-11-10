package com.carriez.flutter_hbb

import android.content.Context
import android.graphics.Rect
import android.os.Build
import android.util.Log
import android.view.accessibility.AccessibilityNodeInfo
import java.io.IOException
import java.util.concurrent.atomic.AtomicReference

private const val SNAPSHOT_TAG = "a11y snapshot"

private val capsSlot = AtomicReference<A11yCaps?>(null)

internal fun peerCaps(context: Context): A11yCaps? {
    capsSlot.get()?.let { return it }
    val json = try {
        context.assets.open("a11y_snapshot_caps.json").bufferedReader(Charsets.UTF_8).use { it.readText() }
    } catch (e: IOException) {
        Log.e(SNAPSHOT_TAG, "snapshot caps unreadable", e)
        return null
    }
    val caps = parseA11yCaps(json)
    if (caps == null) {
        Log.e(SNAPSHOT_TAG, "snapshot caps rejected")
        return null
    }
    capsSlot.compareAndSet(null, caps)
    return capsSlot.get()
}

internal fun semanticSnapshot(context: Context, root: AccessibilityNodeInfo?): String {
    val caps = peerCaps(context) ?: return "snapshot_failed"
    if (root == null) {
        Log.e(SNAPSHOT_TAG, "snapshot has no active window")
        return "snapshot_failed"
    }
    return try {
        val wire = walkA11y(AndroidNode(root), caps)
        "ok\n${wire.json}"
    } catch (e: RuntimeException) {
        Log.e(SNAPSHOT_TAG, "snapshot walk failed", e)
        "snapshot_failed"
    } finally {
        if (Build.VERSION.SDK_INT < 33) root.recycle()
    }
}

internal fun semanticClick(context: Context, root: AccessibilityNodeInfo?, arg: String): String {
    val caps = peerCaps(context) ?: return "snapshot_failed"
    if (root == null) {
        Log.e(SNAPSHOT_TAG, "click has no active window")
        return "snapshot_failed"
    }
    val targets = parseClickTargets(arg, caps)
    if (targets == null) {
        Log.e(SNAPSHOT_TAG, "click target list rejected")
        if (Build.VERSION.SDK_INT < 33) root.recycle()
        return "snapshot_failed"
    }
    val view = AndroidNode(root)
    try {
        for (target in targets) {
            val code = applyClick(view, target)
            if (code != "ok") {
                if (code == "ref_stale") {
                    Log.e(SNAPSHOT_TAG, "click bounds changed")
                } else {
                    Log.e(SNAPSHOT_TAG, "click action failed")
                }
                return code
            }
        }
        return "ok"
    } catch (e: RuntimeException) {
        Log.e(SNAPSHOT_TAG, "click failed", e)
        return "snapshot_failed"
    } finally {
        if (Build.VERSION.SDK_INT < 33) root.recycle()
    }
}

private class AndroidNode(private val info: AccessibilityNodeInfo) : A11yNodeView {
    override fun classPrefix(maxBytes: Int): Prefix = prefixOf(info.className, maxBytes)
    override fun textPrefix(maxBytes: Int): Prefix = prefixOf(info.text, maxBytes)
    override fun descPrefix(maxBytes: Int): Prefix = prefixOf(info.contentDescription, maxBytes)
    override fun idPrefix(maxBytes: Int): Prefix = prefixOf(info.viewIdResourceName, maxBytes)
    override fun pkgPrefix(maxBytes: Int): Prefix = prefixOf(info.packageName, maxBytes)
    override val clickable: Boolean
        get() = info.isClickable
    override val bounds: IntArray
        get() {
            val rect = Rect()
            info.getBoundsInScreen(rect)
            return intArrayOf(rect.left, rect.top, rect.right, rect.bottom)
        }
    override val childCount: Int
        get() = info.childCount
    override fun child(index: Int): A11yNodeView? {
        val child = info.getChild(index) ?: return null
        return AndroidNode(child)
    }
    override fun performClick(): Boolean = info.performAction(AccessibilityNodeInfo.ACTION_CLICK)
    override fun recycle() {
        if (Build.VERSION.SDK_INT < 33) info.recycle()
    }
}
