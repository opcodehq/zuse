package sh.zuse.core

import kotlinx.serialization.json.*

enum class ToolIcon {
    Terminal,
    Read,
    Edit,
    Search,
    Web,
    Tasks,
    Plugin,
    Agent,
    Thinking,
    Other,
}

/** One line of agent activity: what kind of work, and what it touched. */
data class ToolStep(val label: String, val detail: String, val icon: ToolIcon)

/**
 * Names work the way the desktop does (`zuse_protocol::view`), so a turn reads the same on every
 * device. Rust hosts send a typed call; legacy hosts send a tool name and its raw input.
 */
object ToolPresentation {
    private fun oneLine(text: String) = text.trim().split(Regex("\\s+")).joinToString(" ")

    private fun step(label: String, detail: String, icon: ToolIcon) =
        ToolStep(label, oneLine(detail), icon)

    private fun JsonElement.first(vararg keys: String): String =
        keys.firstNotNullOfOrNull { text(it) }.orEmpty()

    fun step(message: Message): ToolStep {
        if (message.kind == "thinking" || message.kind == "reasoning")
            return step("Thinking", "", ToolIcon.Thinking)
        if (message.kind == "tool_result") return step("Output", "", ToolIcon.Other)
        val input = message.content.field("input")
        return if (input.text("kind") != null) typed(input)
        else named(message.content.text("tool") ?: "Tool", input)
    }

    private fun typed(call: JsonElement): ToolStep =
        when (call.text("kind")) {
            "exec" -> step("Run", call.first("command"), ToolIcon.Terminal)
            "readFile" -> step("Read", call.first("path"), ToolIcon.Read)
            "writeFile" -> step("Write", call.first("path"), ToolIcon.Edit)
            "editFile" -> step("Edit", call.first("path"), ToolIcon.Edit)
            "applyPatch" -> step("Patch", call.text("path") ?: "workspace", ToolIcon.Edit)
            "search" -> {
                val pattern = call.first("pattern")
                val path = call.text("path")
                step("Search", if (path != null) "$pattern in $path" else pattern, ToolIcon.Search)
            }
            "glob" -> step("Glob", call.first("pattern"), ToolIcon.Search)
            "webFetch" -> step("Fetch", call.first("url"), ToolIcon.Web)
            "webSearch" -> step("Web", call.first("query"), ToolIcon.Web)
            "todo" -> {
                val items = call.array("items")
                step(
                    "Todo",
                    "${items.count { it.flag("done") }}/${items.size} done",
                    ToolIcon.Tasks,
                )
            }
            "mcp" -> step("MCP", "${call.first("server")} · ${call.first("tool")}", ToolIcon.Plugin)
            else -> {
                val name = call.first("name")
                when {
                    name.startsWith("Agent: ") ->
                        step("Agent", name.removePrefix("Agent: "), ToolIcon.Agent)
                    name == "Agent" -> step("Agent", "", ToolIcon.Agent)
                    else -> step("Tool", name, ToolIcon.Other)
                }
            }
        }

    private fun named(name: String, input: JsonElement): ToolStep {
        val path = input.first("file_path", "path")
        return when (name) {
            "Bash",
            "Run command" -> step("Run", input.first("command"), ToolIcon.Terminal)
            "Read",
            "Read file" -> step("Read", path, ToolIcon.Read)
            "Write",
            "Write file" -> step("Write", path, ToolIcon.Edit)
            "Edit",
            "MultiEdit",
            "Edit file" -> step("Edit", path, ToolIcon.Edit)
            "Grep",
            "Search files" -> step("Search", input.first("pattern"), ToolIcon.Search)
            "Glob",
            "Find files" -> step("Glob", input.first("pattern"), ToolIcon.Search)
            "WebFetch" -> step("Fetch", input.first("url"), ToolIcon.Web)
            "WebSearch" -> step("Web", input.first("query"), ToolIcon.Web)
            "TodoWrite" -> step("Todo", "", ToolIcon.Tasks)
            "Task" -> step("Agent", input.first("description"), ToolIcon.Agent)
            else -> step("Tool", name, ToolIcon.Other)
        }
    }

    private fun plural(count: Int, one: String, many: String) =
        "$count ${if (count == 1) one else many}"

    /** "Ran 3 commands · edited 2 files · 1 failed", the line settled work folds behind. */
    fun summary(rows: List<TranscriptRow>): String {
        val tools = rows.filter { it.message.kind == "tool_use" }
        if (tools.isEmpty()) return if (rows.any { it.isThinking }) "Thought" else "Activity"
        val edited = mutableSetOf<String>()
        var commands = 0
        var reads = 0
        var searches = 0
        var fetches = 0
        var todos = 0
        var other = 0
        for (row in tools) {
            val step = step(row.message)
            when (step.label) {
                "Run" -> commands++
                "Write",
                "Edit",
                "Patch" -> edited += step.detail
                "Read" -> reads++
                "Search",
                "Glob",
                "Web" -> searches++
                "Fetch" -> fetches++
                "Todo" -> todos++
                else -> other++
            }
        }
        val segments = buildList {
            if (commands > 0) add("ran ${plural(commands, "command", "commands")}")
            if (edited.isNotEmpty()) add("edited ${plural(edited.size, "file", "files")}")
            if (reads > 0) add("read ${plural(reads, "file", "files")}")
            if (searches > 0) add("searched ${plural(searches, "time", "times")}")
            if (fetches > 0) add("fetched ${plural(fetches, "page", "pages")}")
            if (todos > 0) add("updated todos")
            if (other > 0) add("called ${plural(other, "tool", "tools")}")
            val failed = tools.count { it.failed }
            if (failed > 0) add("$failed failed")
        }
        return segments.joinToString(" · ").replaceFirstChar(Char::uppercaseChar)
    }

    /** What the agent is doing right now, for the line above work still in progress. */
    fun liveTitle(rows: List<TranscriptRow>): String {
        val current = rows.lastOrNull { it.running } ?: rows.lastOrNull()
        if (current == null || current.isThinking) return "Thinking…"
        val step = step(current.message)
        val verb =
            when (step.label) {
                "Run" -> "Running"
                "Read" -> "Reading"
                "Write" -> "Writing"
                "Edit",
                "Patch" -> "Editing"
                "Search",
                "Glob",
                "Web" -> "Searching"
                "Fetch" -> "Fetching"
                "Todo" -> "Updating todos"
                "Agent" -> "Running agent"
                else -> "Working"
            }
        return if (step.label == "Todo" || step.detail.isEmpty()) "$verb…"
        else "$verb ${step.detail}"
    }
}

/** A tool call joined to its output, when a host reports them separately. */
data class TranscriptRow(val message: Message, val result: Message? = null) {
    val id: String
        get() = message.id

    val isThinking: Boolean
        get() = message.kind == "thinking" || message.kind == "reasoning"

    val isActivity: Boolean
        get() = isThinking || message.kind == "tool_use" || message.kind == "tool_result"

    val failed: Boolean
        get() = message.content.flag("isError") || result?.content?.flag("isError") == true

    /** Rust hosts mark a call resolved; legacy hosts send a separate result. */
    val running: Boolean
        get() =
            message.kind == "tool_use" &&
                result == null &&
                (message.content.field("resolved") as? JsonPrimitive)?.booleanOrNull == false
}

/** Consecutive activity is one group; narrative messages stand alone. */
data class TranscriptGroup(val rows: List<TranscriptRow>) {
    val id: String
        get() = rows.first().id

    val isActivity: Boolean
        get() = rows.first().isActivity

    val hasRunningTool: Boolean
        get() = rows.any { it.running }

    companion object {
        /** Accounting events are metadata, not conversation. */
        private val hiddenKinds = setOf("usage", "context_usage")

        private fun key(message: Message): String? =
            message.content.text("itemId")?.let {
                "${message.content.text("parentItemId").orEmpty()}:$it"
            }

        fun project(messages: List<Message>): List<TranscriptGroup> {
            val outputs = mutableMapOf<String, IndexedValue<Message>>()
            messages.withIndex().forEach { (index, message) ->
                if (message.kind == "tool_result")
                    key(message)?.let { outputs[it] = IndexedValue(index, message) }
            }
            val paired = mutableMapOf<String, Message>()
            val consumed = mutableSetOf<String>()
            messages.withIndex().forEach { (index, message) ->
                if (message.kind != "tool_use") return@forEach
                val output = key(message)?.let { outputs[it] } ?: return@forEach
                if (output.index > index && consumed.add(output.value.id))
                    paired[message.id] = output.value
            }
            val groups = mutableListOf<MutableList<TranscriptRow>>()
            for (message in messages) {
                if (message.id in consumed || message.kind in hiddenKinds) continue
                val row = TranscriptRow(message, paired[message.id])
                if (row.isActivity && groups.lastOrNull()?.lastOrNull()?.isActivity == true)
                    groups.last().add(row)
                else groups.add(mutableListOf(row))
            }
            return groups.map { TranscriptGroup(it) }
        }
    }
}
