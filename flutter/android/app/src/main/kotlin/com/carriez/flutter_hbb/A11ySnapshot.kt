package com.carriez.flutter_hbb

import java.util.ArrayDeque

/**
 * Accessibility snapshot limits and the tree walk. No Android types, so the
 * unit test can run without an SDK.
 *
 * Numbers come only from `a11y_snapshot_caps.json`. A walk stops at the node
 * cap, the clickable-ref cap, or the payload cap and marks `reason`. It does
 * not launch an activity.
 */

internal class A11yCaps(
    val maxRefs: Int,
    val maxNodes: Int,
    val payloadMaxBytes: Int,
    val snapshotTimeoutMs: Long,
    val clickTimeoutMs: Long,
    val refTtlMs: Long,
    val batchMax: Int,
)

internal class Prefix(val text: String, val cut: Boolean)

internal interface A11yNodeView {
    fun classPrefix(maxBytes: Int): Prefix
    fun textPrefix(maxBytes: Int): Prefix
    fun descPrefix(maxBytes: Int): Prefix
    fun idPrefix(maxBytes: Int): Prefix
    fun pkgPrefix(maxBytes: Int): Prefix
    val clickable: Boolean
    val bounds: IntArray
    val childCount: Int
    fun child(index: Int): A11yNodeView?
    fun performClick(): Boolean
    fun recycle()
}

internal class ClickTarget(val path: IntArray, val bounds: IntArray)

internal class WalkWire(
    val json: String,
    val reason: String,
    val nodeCount: Int,
)

internal fun parseA11yCaps(json: String): A11yCaps? {
    val body = json.trim()
    if (body.length < 2 || body.first() != '{' || body.last() != '}') return null
    val parts = body.substring(1, body.length - 1).split(',')
    if (parts.size != 7) return null
    val map = HashMap<String, Long>(8)
    for (part in parts) {
        val pair = part.split(':', limit = 2)
        if (pair.size != 2) return null
        val key = jsonKey(pair[0]) ?: return null
        val value = pair[1].trim().toLongOrNull() ?: return null
        if (value <= 0L || value > Int.MAX_VALUE) return null
        if (map.put(key, value) != null) return null
    }
    if (map.size != 7) return null
    return A11yCaps(
        maxRefs = map["max_refs"]?.toInt() ?: return null,
        maxNodes = map["max_nodes"]?.toInt() ?: return null,
        payloadMaxBytes = map["payload_max_bytes"]?.toInt() ?: return null,
        snapshotTimeoutMs = map["snapshot_timeout_ms"] ?: return null,
        clickTimeoutMs = map["click_timeout_ms"] ?: return null,
        refTtlMs = map["ref_ttl_ms"] ?: return null,
        batchMax = map["batch_max"]?.toInt() ?: return null,
    )
}

internal fun walkA11y(root: A11yNodeView, caps: A11yCaps): WalkWire {
    val parts = ArrayList<String>(minOf(caps.maxNodes, 32))
    var reason = ""
    var stopped = false
    var clickableKept = 0
    val reserve = envelopeBytes("payload")
    var used = reserve
    val stack = ArrayDeque<Visit>()
    stack.add(Visit(root, IntArray(0), 0, false, owned = false))
    while (stack.isNotEmpty()) {
        val top = stack.last()
        if (!top.entered) {
            top.entered = true
            if (parts.size >= caps.maxNodes) {
                reason = "nodes"
                stopped = true
                break
            }
            if (top.node.clickable && clickableKept >= caps.maxRefs) {
                reason = "refs"
                stopped = true
                break
            }
            val room = caps.payloadMaxBytes - used - if (parts.isEmpty()) 0 else 1
            val rendered = renderWithin(top.node, top.path, room)
            if (rendered == null) {
                reason = "payload"
                stopped = true
                break
            }
            used += rendered.json.toByteArray(Charsets.UTF_8).size + if (parts.isEmpty()) 0 else 1
            parts.add(rendered.json)
            if (top.node.clickable) clickableKept += 1
            if (rendered.cut) {
                reason = "payload"
                stopped = true
                break
            }
        }
        if (parts.size >= caps.maxNodes) {
            reason = "nodes"
            stopped = true
            break
        }
        if (top.nextChild < top.node.childCount) {
            val index = top.nextChild
            top.nextChild += 1
            val child = top.node.child(index) ?: continue
            val path = IntArray(top.path.size + 1)
            top.path.copyInto(path)
            path[path.size - 1] = index
            stack.add(Visit(child, path, 0, false, owned = true))
        } else {
            val done = stack.removeLast()
            if (done.owned) done.node.recycle()
        }
    }
    while (stack.isNotEmpty()) {
        val done = stack.removeLast()
        if (done.owned) done.node.recycle()
    }
    if (!stopped) reason = ""
    return WalkWire(assembleWire(reason, parts), reason, parts.size)
}

internal fun clickBoundsCode(expected: IntArray, actual: IntArray): String {
    if (expected.size != 4 || actual.size != 4 || !expected.contentEquals(actual)) {
        return "ref_stale"
    }
    return "ok"
}

internal fun applyClick(root: A11yNodeView, target: ClickTarget): String {
    val node = nodeAt(root, target.path) ?: return "snapshot_failed"
    val code = clickBoundsCode(target.bounds, node.bounds)
    if (code != "ok") {
        if (node !== root) node.recycle()
        return code
    }
    val clicked = node.performClick()
    if (node !== root) node.recycle()
    return if (clicked) "ok" else "snapshot_failed"
}

internal fun nodeAt(root: A11yNodeView, path: IntArray): A11yNodeView? {
    var current = root
    var owned = false
    for (index in path) {
        val child = current.child(index)
        if (child == null) {
            if (owned) current.recycle()
            return null
        }
        if (owned) current.recycle()
        current = child
        owned = true
    }
    return current
}

internal fun parseClickTargets(arg: String, caps: A11yCaps): List<ClickTarget>? {
    val trimmed = arg.trim()
    if (trimmed.startsWith("{")) {
        val one = parseOneTarget(trimmed, caps.maxNodes) ?: return null
        return listOf(one)
    }
    val items = splitTargets(trimmed) ?: return null
    if (items.isEmpty() || items.size > caps.batchMax) return null
    val out = ArrayList<ClickTarget>(items.size)
    for (item in items) {
        out.add(parseOneTarget(item.trim(), caps.maxNodes) ?: return null)
    }
    return out
}

internal fun prefixOf(raw: CharSequence?, maxBytes: Int): Prefix {
    if (raw == null || raw.isEmpty()) return Prefix("", cut = false)
    if (maxBytes <= 0) return Prefix("", cut = true)
    val sb = StringBuilder()
    var used = 0
    var i = 0
    while (i < raw.length && used < maxBytes) {
        val cp = Character.codePointAt(raw, i)
        val size = when {
            cp <= 0x7F -> 1
            cp <= 0x7FF -> 2
            cp <= 0xFFFF -> 3
            else -> 4
        }
        if (used + size > maxBytes) return Prefix(sb.toString(), cut = true)
        sb.appendCodePoint(cp)
        used += size
        i += Character.charCount(cp)
    }
    return Prefix(sb.toString(), cut = i < raw.length)
}

private class Visit(
    val node: A11yNodeView,
    val path: IntArray,
    var nextChild: Int,
    var entered: Boolean,
    val owned: Boolean,
)

private class Rendered(val json: String, val cut: Boolean)

private fun renderWithin(node: A11yNodeView, path: IntArray, budget: Int): Rendered? {
    if (budget <= 0) return null
    val cls = node.classPrefix(budget)
    val text = node.textPrefix(budget)
    val desc = node.descPrefix(budget)
    val id = node.idPrefix(budget)
    val pkg = node.pkgPrefix(budget)
    val full = nodeJson(cls.text, text.text, desc.text, id.text, pkg.text, node, path)
    if (utf8(full) <= budget) {
        val cut = cls.cut || text.cut || desc.cut || id.cut || pkg.cut
        return Rendered(full, cut)
    }
    val fitted = shrinkText(text.text, budget) { prefix ->
        nodeJson(cls.text, prefix, desc.text, id.text, pkg.text, node, path)
    }
    if (fitted != null) return Rendered(fitted, cut = true)
    val emptied = nodeJson("", "", "", "", "", node, path)
    if (utf8(emptied) <= budget) return Rendered(emptied, cut = true)
    return null
}

private fun shrinkText(text: String, budget: Int, render: (String) -> String): String? {
    var lo = 0
    var hi = text.length
    var best: String? = null
    while (lo <= hi) {
        val mid = lo + (hi - lo) / 2
        val json = render(text.substring(0, mid))
        if (utf8(json) <= budget) {
            best = json
            if (mid == hi) break
            lo = mid + 1
        } else if (mid == 0) {
            break
        } else {
            hi = mid - 1
        }
    }
    return best
}

private fun nodeJson(
    cls: String,
    text: String,
    desc: String,
    id: String,
    pkg: String,
    node: A11yNodeView,
    path: IntArray,
): String {
    val sb = StringBuilder()
    sb.append("{\"cls\":")
    appendJson(sb, cls)
    sb.append(",\"text\":")
    appendJson(sb, text)
    sb.append(",\"desc\":")
    appendJson(sb, desc)
    sb.append(",\"id\":")
    appendJson(sb, id)
    sb.append(",\"pkg\":")
    appendJson(sb, pkg)
    sb.append(",\"click\":")
    sb.append(if (node.clickable) "true" else "false")
    sb.append(",\"b\":[")
    node.bounds.forEachIndexed { i, v ->
        if (i > 0) sb.append(',')
        sb.append(v)
    }
    sb.append("],\"path\":[")
    path.forEachIndexed { i, v ->
        if (i > 0) sb.append(',')
        sb.append(v)
    }
    sb.append("]}")
    return sb.toString()
}

private fun assembleWire(reason: String, parts: List<String>): String {
    val sb = StringBuilder()
    sb.append("{\"truncated\":")
    sb.append(if (reason.isEmpty()) "false" else "true")
    sb.append(",\"reason\":")
    appendJson(sb, reason)
    sb.append(",\"nodes\":[")
    parts.forEachIndexed { i, part ->
        if (i > 0) sb.append(',')
        sb.append(part)
    }
    sb.append("]}")
    return sb.toString()
}

private fun envelopeBytes(reason: String): Int {
    val flag = if (reason.isEmpty()) "false" else "true"
    return utf8("{\"truncated\":") + flag.length + utf8(",\"reason\":") + 2 + utf8(reason) +
        utf8(",\"nodes\":[]}")
}

private fun utf8(value: String): Int = value.toByteArray(Charsets.UTF_8).size

private fun appendJson(sb: StringBuilder, value: String) {
    sb.append('"')
    for (ch in value) {
        when (ch) {
            '\\' -> sb.append("\\\\")
            '"' -> sb.append("\\\"")
            '\n' -> sb.append("\\n")
            '\r' -> sb.append("\\r")
            '\t' -> sb.append("\\t")
            else -> {
                if (ch.code < 0x20) sb.append("\\u%04x".format(ch.code)) else sb.append(ch)
            }
        }
    }
    sb.append('"')
}

private fun jsonKey(raw: String): String? {
    val text = raw.trim()
    if (text.length < 2 || text.first() != '"' || text.last() != '"') return null
    val key = text.substring(1, text.length - 1)
    if (key.isEmpty() || key.any { it == '"' || it == '\\' }) return null
    return key
}

private fun splitTargets(arg: String): List<String>? {
    if (arg.length < 2 || arg.first() != '[' || arg.last() != ']') return null
    val body = arg.substring(1, arg.length - 1).trim()
    if (body.isEmpty()) return null
    val items = ArrayList<String>()
    val sb = StringBuilder()
    var depth = 0
    for (ch in body) {
        if (ch == '{') depth += 1
        if (ch == '}') depth -= 1
        if (depth < 0) return null
        if (ch == ',' && depth == 0) {
            items.add(sb.toString())
            sb.setLength(0)
        } else {
            sb.append(ch)
        }
    }
    if (depth != 0) return null
    items.add(sb.toString())
    return items
}

private fun parseOneTarget(item: String, maxPath: Int): ClickTarget? {
    val prefix = "{\"path\":["
    val mid = "],\"b\":["
    val end = "]}"
    if (!item.startsWith(prefix) || !item.endsWith(end)) return null
    val rest = item.substring(prefix.length, item.length - end.length)
    val parts = rest.split(mid)
    if (parts.size != 2) return null
    val path = parseInts(parts[0], maxPath, allowNegative = false) ?: return null
    val bounds = parseInts(parts[1], 4, allowNegative = true) ?: return null
    if (bounds.size != 4) return null
    return ClickTarget(path, bounds)
}

private fun parseInts(body: String, maxCount: Int, allowNegative: Boolean): IntArray? {
    if (body.isEmpty()) return IntArray(0)
    val bits = body.split(',')
    if (bits.isEmpty() || bits.size > maxCount) return null
    val out = IntArray(bits.size)
    for (i in bits.indices) {
        val n = bits[i].trim().toIntOrNull() ?: return null
        if (!allowNegative && n < 0) return null
        out[i] = n
    }
    return out
}
