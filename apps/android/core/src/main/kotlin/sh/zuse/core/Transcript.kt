package sh.zuse.core

import kotlinx.serialization.json.*

/** Identity-indexed transcript; history is paged and live upserts are O(1). Single owner only. */
class Transcript {
    private val rows = ArrayList<Message>()
    private val indices = HashMap<String, Int>()
    val messages: List<Message>
        get() = rows

    var version = 0L
        private set

    var epoch: String? = null
        private set

    var hasSnapshot = false
        private set

    var olderSequence: Long? = null
        private set

    var status = "idle"
        private set

    var activeTurnId: String? = null
        private set

    var interactions: List<JsonElement> = emptyList()
        private set

    fun replace(messages: List<Message>) {
        rows.clear()
        indices.clear()
        merge(messages)
    }

    fun restore(messages: List<Message>) {
        merge(messages)
    }

    fun apply(frame: JsonElement, sessionId: String) {
        require(frame.text("sessionId") == sessionId) { "Received a frame for a different chat." }
        val kind = frame.text("kind")
        if (kind == "reset-required")
            throw ClientException("The transcript changed. Reconnect to synchronize.")
        val incomingEpoch = frame.field("cursor").text("epoch")
        if (kind != "snapshot" && epoch != null && incomingEpoch != null && incomingEpoch != epoch)
            throw ClientException("The runtime restarted. Reconnect to synchronize.")
        when (kind) {
            "snapshot" -> {
                val projection = frame.field("projection")
                val decoded = projection.field("messages").decode<List<Message>>()
                require(decoded.all { it.sessionId == sessionId }) {
                    "Snapshot contains another chat."
                }
                rows.clear()
                indices.clear()
                merge(decoded)
                version = frame.long("throughVersion") ?: 0
                epoch = incomingEpoch
                hasSnapshot = true
                olderSequence =
                    frame.long("olderMessageSequence") ?: projection.long("olderMessageSequence")
                status = projection.text("status") ?: "idle"
                activeTurnId = projection.field("activeTurn").text("id")
                interactions = projection.array("interactions")
            }
            "snapshot-chunk" -> {
                require(hasSnapshot && frame.long("throughVersion") == version) {
                    "Missing matching history snapshot."
                }
                merge(frame.field("messages").decode(), prepend = true)
                olderSequence = frame.long("olderMessageSequence")
            }
            "event" -> {
                require(hasSnapshot) { "Missing transcript snapshot." }
                val next =
                    frame.long("streamVersion")
                        ?: throw ClientException("Missing transcript version.")
                if (next <= version) return
                require(next == version + 1) {
                    "Transcript update missed. Reconnect to synchronize."
                }
                val event = frame.field("event")
                when (event.text("_tag")) {
                    "MessagePersisted" -> {
                        val message: Message = event.field("message").decode()
                        require(message.sessionId == sessionId)
                        merge(listOf(message))
                        val item = message.content.text("itemId")
                        if (item != null && message.kind == "user_question")
                            upsert(
                                obj(
                                    "_tag" to "Question".json(),
                                    "id" to item.json(),
                                    "questions" to message.content.field("questions"),
                                )
                            )
                        if (item != null && message.kind == "user_question_answer")
                            remove("Question", item)
                        if (
                            item != null &&
                                message.kind == "tool_use" &&
                                message.content.text("tool") == "ExitPlanMode"
                        )
                            upsert(obj("_tag" to "Plan".json(), "id" to item.json()))
                        if (item != null && message.kind == "tool_result") remove("Plan", item)
                    }
                    "StatusSet" -> status = event.text("status") ?: status
                    "TurnStarted",
                    "TurnUpdated" -> activeTurnId = event.field("turn").text("id")
                    "TurnSettled" -> {
                        activeTurnId = null
                        status = "idle"
                    }
                    "TranscriptRolledBack" -> {
                        val removed =
                            event
                                .array("removedMessageIds")
                                .map { it.jsonPrimitive.content }
                                .toSet()
                        rows.removeAll { it.id in removed }
                        reindex()
                    }
                    "PermissionRequested" -> {
                        val request = event.field("request")
                        upsert(
                            obj(
                                "_tag" to "Permission".json(),
                                "id" to request.field("id"),
                                "request" to request,
                            )
                        )
                    }
                    "PermissionResolved" -> remove("Permission", event.text("requestId"))
                    "QuestionResolved" -> remove("Question", event.text("itemId"))
                    "PermissionExpired",
                    "QuestionExpired",
                    "PlanExpired" -> {
                        val tag = event.text("_tag")!!.removeSuffix("Expired")
                        val id = event.text("requestId") ?: event.text("itemId")
                        interactions =
                            interactions.map {
                                if (it.text("_tag") == tag && it.text("id") == id)
                                    JsonObject(
                                        it.jsonObject + ("recoveryState" to "expired".json())
                                    )
                                else it
                            }
                    }
                }
                version = next
            }
            "synchronized" ->
                require(hasSnapshot && frame.long("throughVersion") == version) {
                    "Transcript synchronization incomplete."
                }
            "runtime" -> status = frame.field("observation").text("state") ?: status
            else -> throw ClientException("Update the app to read this transcript frame.")
        }
    }

    fun prependPage(page: JsonElement, sessionId: String) {
        val messages: List<Message> = page.field("messages").decode()
        require(messages.all { it.sessionId == sessionId })
        merge(messages.reversed(), prepend = true)
        olderSequence = page.long("olderMessageSequence")
    }

    private fun merge(incoming: List<Message>, prepend: Boolean = false) {
        val added = LinkedHashMap<String, Message>()
        for (message in incoming) {
            val index = indices[message.id]
            if (index != null) rows[index] = message else added[message.id] = message
        }
        if (prepend) {
            rows.addAll(0, added.values)
            reindex()
        } else
            for (message in added.values) {
                indices[message.id] = rows.size
                rows.add(message)
            }
    }

    private fun reindex() {
        indices.clear()
        rows.forEachIndexed { i, message -> indices[message.id] = i }
    }

    private fun upsert(value: JsonElement) {
        interactions =
            interactions.filterNot {
                it.text("_tag") == value.text("_tag") && it.text("id") == value.text("id")
            } + value
    }

    private fun remove(tag: String, id: String?) {
        interactions = interactions.filterNot { it.text("_tag") == tag && it.text("id") == id }
    }
}
