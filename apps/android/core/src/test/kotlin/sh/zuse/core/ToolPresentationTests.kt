package sh.zuse.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.json.*

class ToolPresentationTests {
    private var next = 0

    private fun tool(call: JsonObject, resolved: Boolean = true, error: Boolean = false) =
        Message(
            "m${next++}",
            "s",
            "assistant",
            obj(
                "_tag" to "tool_use".json(),
                "tool" to "x".json(),
                "input" to call,
                "resolved" to resolved.json(),
                "isError" to error.json(),
            ),
        )

    private fun kind(kind: String, vararg fields: Pair<String, JsonElement?>) =
        obj("kind" to kind.json(), *fields)

    private fun exec(command: String = "cargo test") = kind("exec", "command" to command.json())

    private fun text(body: String) =
        Message("m${next++}", "s", "assistant", obj("_tag" to "text".json(), "text" to body.json()))

    @Test
    fun namesTypedCallsLikeTheDesktop() {
        fun step(call: JsonObject) = ToolPresentation.step(tool(call))
        assertEquals(ToolStep("Run", "cargo test", ToolIcon.Terminal), step(exec()))
        assertEquals("Read", step(kind("readFile", "path" to "a.rs".json())).label)
        assertEquals(
            "foo in src",
            step(kind("search", "pattern" to "foo".json(), "path" to "src".json())).detail,
        )
        assertEquals("workspace", step(kind("applyPatch")).detail)
        assertEquals(
            "1/2 done",
            step(
                    kind(
                        "todo",
                        "items" to
                            JsonArray(
                                listOf(obj("done" to true.json()), obj("done" to false.json()))
                            ),
                    )
                )
                .detail,
        )
        assertEquals(
            "linear · search",
            step(kind("mcp", "server" to "linear".json(), "tool" to "search".json())).detail,
        )
        assertEquals(
            ToolStep("Agent", "scan repo", ToolIcon.Agent),
            step(kind("unknown", "name" to "Agent: scan repo".json())),
        )
        assertEquals("Tool", step(kind("unknown", "name" to "custom".json())).label)
    }

    @Test
    fun commandsCollapseOntoOneLine() {
        assertEquals("a b c", ToolPresentation.step(tool(exec("a\n  b\tc"))).detail)
    }

    @Test
    fun namesLegacyToolsToo() {
        val message =
            Message(
                "m",
                "s",
                "assistant",
                obj(
                    "_tag" to "tool_use".json(),
                    "tool" to "Edit".json(),
                    "input" to obj("file_path" to "a.rs".json()),
                ),
            )
        assertEquals(ToolStep("Edit", "a.rs", ToolIcon.Edit), ToolPresentation.step(message))
    }

    @Test
    fun summaryMatchesTheDesktopLine() {
        val rows =
            listOf(
                    tool(exec()),
                    tool(exec(), error = true),
                    tool(kind("editFile", "path" to "a.rs".json())),
                    tool(kind("writeFile", "path" to "a.rs".json())),
                    tool(kind("readFile", "path" to "b.rs".json())),
                )
                .map { TranscriptRow(it) }
        assertEquals(
            "Ran 2 commands · edited 1 file · read 1 file · 1 failed",
            ToolPresentation.summary(rows),
        )
    }

    @Test
    fun liveTitleFollowsTheToolStillRunning() {
        val rows =
            listOf(tool(kind("readFile", "path" to "a.rs".json())), tool(exec(), resolved = false))
                .map { TranscriptRow(it) }
        assertEquals("Running cargo test", ToolPresentation.liveTitle(rows))
    }

    @Test
    fun groupsActivityBetweenNarrativeAndHidesAccounting() {
        val messages =
            listOf(
                text("before"),
                tool(exec()),
                Message("u", "s", "assistant", obj("_tag" to "usage".json())),
                tool(exec()),
                text("after"),
            )
        val groups = TranscriptGroup.project(messages)
        assertEquals(listOf(1, 2, 1), groups.map { it.rows.size })
        assertTrue(groups[1].isActivity)
        assertFalse(groups[0].isActivity)
    }

    @Test
    fun legacyResultsJoinTheirCall() {
        val call =
            Message(
                "c",
                "s",
                "assistant",
                obj("_tag" to "tool_use".json(), "tool" to "Bash".json(), "itemId" to "1".json()),
            )
        val result =
            Message(
                "r",
                "s",
                "assistant",
                obj(
                    "_tag" to "tool_result".json(),
                    "itemId" to "1".json(),
                    "isError" to true.json(),
                ),
            )
        val groups = TranscriptGroup.project(listOf(call, result))
        assertEquals(1, groups.single().rows.size)
        assertTrue(groups.single().rows.single().failed)
    }

    @Test
    fun runningOnlyWhenAHostSaysUnresolved() {
        assertTrue(TranscriptRow(tool(exec(), resolved = false)).running)
        assertFalse(TranscriptRow(tool(exec())).running)
    }
}
