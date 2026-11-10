package com.carriez.flutter_hbb

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class A11ySnapshotTest {
    private val caps = parseA11yCaps(File(System.getProperty("a11y.caps")).readText())!!

    @Test
    fun capsFileIsTheContract() {
        assertEquals(80, caps.refMaxLen)
        assertEquals(256, caps.maxNodes)
        assertEquals(32 * 1024, caps.payloadMaxBytes)
        assertEquals(1500L, caps.snapshotTimeoutMs)
        assertEquals(1000L, caps.clickTimeoutMs)
        assertEquals(5000L, caps.refTtlMs)
        assertEquals(8, caps.batchMax)
        assertNull(parseA11yCaps("{\"ref_max_len\": 80}"))
        val extra = File(System.getProperty("a11y.caps")).readText().replace("}", ",\"fg\": 1}")
        assertNull(parseA11yCaps(extra))
    }

    @Test
    fun walkKeepsAButtonAndRecordsItsPath() {
        val child = CountingNode(
            className = "android.widget.TextView",
            text = "外卖",
            bounds = intArrayOf(0, 40, 20, 60),
        )
        val root = CountingNode(
            className = "android.widget.Button",
            text = "搜索",
            viewId = "com.sankuai.meituan:id/search",
            packageName = "com.sankuai.meituan",
            clickable = true,
            bounds = intArrayOf(0, 10, 20, 30),
            children = listOf(child),
        )
        val wire = walkA11y(root, caps)
        assertEquals("", wire.reason)
        assertEquals(2, wire.nodeCount)
        assertEquals(
            "{\"truncated\":false,\"reason\":\"\",\"nodes\":[" +
                "{\"cls\":\"android.widget.Button\",\"text\":\"搜索\",\"desc\":\"\"," +
                "\"id\":\"com.sankuai.meituan:id/search\",\"pkg\":\"com.sankuai.meituan\"," +
                "\"click\":true,\"b\":[0,10,20,30],\"path\":[]}," +
                "{\"cls\":\"android.widget.TextView\",\"text\":\"外卖\",\"desc\":\"\"," +
                "\"id\":\"\",\"pkg\":\"\",\"click\":false,\"b\":[0,40,20,60],\"path\":[0]}" +
                "]}",
            wire.json,
        )
        assertTrue(wire.json.toByteArray(Charsets.UTF_8).size <= caps.payloadMaxBytes)
        assertEquals(1, root.childCalls)
        assertFalse(root.recycled)
        assertTrue(child.recycled)
        val found = nodeAt(root, intArrayOf(0))
        assertNotNull(found)
        assertTrue(found!!.bounds.contentEquals(intArrayOf(0, 40, 20, 60)))
        assertNull(nodeAt(root, intArrayOf(1)))
    }

    @Test
    fun nodeCapDoesNotReadPastTheLimit() {
        val kids = ArrayList<CountingNode>(300)
        for (i in 0 until 300) {
            kids.add(CountingNode(text = "t$i", bounds = intArrayOf(0, i, 1, i + 1)))
        }
        val root = CountingNode(text = "root", children = kids)
        val wire = walkA11y(root, caps)
        assertEquals("nodes", wire.reason)
        assertEquals(caps.maxNodes, wire.nodeCount)
        assertTrue(wire.json.contains("\"truncated\":true"))
        assertTrue(wire.json.toByteArray(Charsets.UTF_8).size <= caps.payloadMaxBytes)
        assertEquals(caps.maxNodes - 1, root.childCalls)
        assertFalse(wire.json.contains("\"text\":\"t${caps.maxNodes}\""))
    }

    @Test
    fun payloadIsTruncatedAndMarked() {
        val root = CountingNode(text = "x".repeat(40_000), bounds = intArrayOf(0, 0, 1, 1))
        val wire = walkA11y(root, caps)
        assertEquals("payload", wire.reason)
        assertTrue(wire.json.contains("\"truncated\":true"))
        assertTrue(wire.json.toByteArray(Charsets.UTF_8).size <= caps.payloadMaxBytes)
        assertTrue(wire.nodeCount >= 1)
    }

    @Test
    fun batchOverTheCapIsRejected() {
        val one = "{\"path\":[0],\"b\":[0,1,2,3]}"
        val eight = "[" + (1..8).joinToString(",") { one } + "]"
        val nine = "[" + (1..9).joinToString(",") { one } + "]"
        assertEquals(8, parseClickTargets(eight, caps)!!.size)
        assertNull(parseClickTargets(nine, caps))
        assertNull(parseClickTargets("[]", caps))
        assertNull(parseClickTargets("{\"path\":[0],\"b\":[0]}", caps))
    }
}

private class CountingNode(
    private val className: String = "c",
    private val text: String = "",
    private val desc: String = "",
    private val viewId: String = "",
    private val packageName: String = "",
    override val clickable: Boolean = false,
    override val bounds: IntArray = intArrayOf(0, 0, 1, 1),
    private val children: List<CountingNode> = emptyList(),
) : A11yNodeView {
    var childCalls: Int = 0
    var recycled: Boolean = false

    override fun classPrefix(maxBytes: Int) = prefixOf(className, maxBytes)
    override fun textPrefix(maxBytes: Int) = prefixOf(text, maxBytes)
    override fun descPrefix(maxBytes: Int) = prefixOf(desc, maxBytes)
    override fun idPrefix(maxBytes: Int) = prefixOf(viewId, maxBytes)
    override fun pkgPrefix(maxBytes: Int) = prefixOf(packageName, maxBytes)
    override val childCount: Int
        get() = children.size
    override fun child(index: Int): A11yNodeView? {
        childCalls += 1
        return children.getOrNull(index)
    }
    override fun performClick(): Boolean = clickable
    override fun recycle() {
        recycled = true
    }
}
