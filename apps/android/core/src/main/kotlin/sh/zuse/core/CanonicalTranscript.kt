package sh.zuse.core

import kotlinx.serialization.json.*

/** Rust WatchDocMessages reset/delta fold. Validate the whole update before publishing it. */
class CanonicalTranscript {
    private var entries = linkedMapOf<String, JsonElement>()
    private var initialized = false
    private var renderedSession: String? = null
    private val rendered = HashMap<String, Pair<JsonElement, List<Message>>>()

    fun interactions(): List<JsonElement> =
        entries.values
            .flatMap { entry ->
                entry
                    .array("parts")
                    .filter { it.text("kind") == "input" && !it.flag("resolved") }
                    .map { part ->
                        obj(
                            "_tag" to "Question".json(),
                            "id" to part.field("requestId"),
                            "questions" to part.field("questions"),
                        )
                    }
            }
            .distinctBy { it.text("id") }

    fun messages(sessionId: String): List<Message> {
        if (renderedSession != sessionId) {
            rendered.clear()
            renderedSession = sessionId
        }
        rendered.keys.retainAll(entries.keys)
        return entries.values.flatMap { entry ->
            val id = requireNotNull(entry.text("id"))
            val cached = rendered[id]
            if (cached != null && cached.first === entry) return@flatMap cached.second
            val rows =
                entry.array("parts").mapNotNull { part ->
                    val kind = part.text("kind") ?: return@mapNotNull null
                    if (kind == "agentContext" || kind == "input") return@mapNotNull null
                    val content =
                        when (kind) {
                            "tool" -> {
                                val call = part.field("call")
                                val label =
                                    when (call.text("kind")) {
                                        "exec" -> "Run command"
                                        "readFile" -> "Read file"
                                        "writeFile" -> "Write file"
                                        "editFile",
                                        "applyPatch" -> "Edit file"
                                        "search" -> "Search files"
                                        "glob" -> "Find files"
                                        "webFetch" -> "Read website"
                                        "webSearch" -> "Search web"
                                        "todo" -> "Update tasks"
                                        "mcp" ->
                                            listOfNotNull(call.text("server"), call.text("tool"))
                                                .joinToString(" · ")
                                        else -> call.text("name") ?: "Tool"
                                    }
                                obj(
                                    "_tag" to "tool_use".json(),
                                    "tool" to label.json(),
                                    "input" to call,
                                    "output" to part.field("output"),
                                    "text" to
                                        listOfNotNull(
                                                call.text("command"),
                                                call.text("path"),
                                                call.text("pattern"),
                                                call.text("url"),
                                                call.text("query"),
                                                part.text("output"),
                                            )
                                            .joinToString("\n")
                                            .ifEmpty { call.toString() }
                                            .json(),
                                    "resolved" to part.field("resolved"),
                                    "isError" to part.field("isError"),
                                )
                            }
                            "error" -> obj("_tag" to "text".json(), "text" to part.field("message"))
                            else -> JsonObject(part.jsonObject + ("_tag" to kind.json()))
                        }
                    Message(
                        "${entry.text("id")}:${part.text("id")}",
                        sessionId,
                        entry.text("role") ?: "assistant",
                        content,
                    )
                }
            rendered[id] = entry to rows
            rows
        }
    }

    fun apply(update: JsonElement): List<JsonElement> {
        val fields = update.jsonObject
        val next = LinkedHashMap(entries)
        if ("reset" in fields) {
            next.clear()
            update.array("reset").forEach { entry ->
                val id = requireNotNull(entry.text("id"))
                require(next.put(id, entry) == null) { "Duplicate transcript entry" }
            }
        } else {
            require(initialized) { "Missing transcript reset" }
            update.array("remove").forEach { next.remove(it.jsonPrimitive.content) }
            update.array("upsert").forEach { change ->
                val entry = change.field("entry")
                val id = requireNotNull(entry.text("id"))
                val anchor = change.text("after")
                next.remove(id)
                require(anchor == null || anchor in next) { "Missing transcript anchor" }
                val ordered = LinkedHashMap<String, JsonElement>()
                if (anchor == null) ordered[id] = entry
                next.forEach { (key, value) ->
                    ordered[key] = value
                    if (key == anchor) ordered[id] = entry
                }
                next.clear()
                next.putAll(ordered)
            }
            update.array("append").forEach { append ->
                val id = requireNotNull(append.text("entry"))
                val entry = requireNotNull(next[id]) { "Missing transcript entry" }
                var found = false
                val parts =
                    entry.array("parts").map { part ->
                        if (part.text("id") != append.text("part")) part
                        else {
                            require(part.text("kind") in listOf("text", "reasoning"))
                            val text =
                                requireNotNull(part.text("text")) +
                                    requireNotNull(append.text("text"))
                            require(
                                text.toByteArray(Charsets.UTF_8).size.toLong() == append.long("len")
                            ) {
                                "Transcript append length mismatch"
                            }
                            found = true
                            JsonObject(part.jsonObject + ("text" to text.json()))
                        }
                    }
                require(found) { "Missing transcript part" }
                next[id] = JsonObject(entry.jsonObject + ("parts" to JsonArray(parts)))
            }
            require(next.size.toLong() == update.long("count")) { "Transcript count mismatch" }
        }
        entries = next
        initialized = true
        return next.values.toList()
    }
}
