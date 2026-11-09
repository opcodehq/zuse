package sh.zuse.core

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.*

val WireJson = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
}

fun obj(vararg fields: Pair<String, JsonElement?>): JsonObject =
    JsonObject(fields.filter { it.second != null }.associate { it.first to it.second!! })

fun String.json() = JsonPrimitive(this)

fun Boolean.json() = JsonPrimitive(this)

fun Number.json() = JsonPrimitive(this)

fun JsonElement.field(key: String): JsonElement = (this as? JsonObject)?.get(key) ?: JsonNull

fun JsonElement.text(key: String): String? = (field(key) as? JsonPrimitive)?.contentOrNull

fun JsonElement.long(key: String): Long? = (field(key) as? JsonPrimitive)?.longOrNull

fun JsonElement.flag(key: String): Boolean = (field(key) as? JsonPrimitive)?.booleanOrNull == true

fun JsonElement.array(key: String): List<JsonElement> = field(key) as? JsonArray ?: emptyList()

inline fun <reified T> JsonElement.decode(): T = WireJson.decodeFromJsonElement(this)

@Serializable data class Project(val id: String, val name: String)

@Serializable
data class Session(
    val id: String,
    val projectId: String,
    val title: String = "New chat",
    val providerId: String,
    val model: String,
    val worktreeId: String? = null,
)

@Serializable
data class Message(
    val id: String,
    val sessionId: String,
    val role: String,
    val content: JsonElement,
    val createdAt: String = "",
) {
    val kind: String
        get() = content.text("_tag") ?: role

    val text: String
        get() =
            content.text("text")
                ?: content.text("message")
                ?: content.text("output")
                ?: WireJson.encodeToString(JsonElement.serializer(), content)
}

@Serializable
data class SavedDevice(
    val id: String,
    val label: String,
    val address: String,
    val token: String,
    val environmentId: String? = null,
    val certificatePin: String? = null,
    val pairingLink: String? = null,
)

@Serializable
data class CloudChat(
    val workspaceId: String,
    val projectId: String,
    val chatId: String,
    val initialSessionId: String,
    val activeSessionId: String? = null,
    val title: String,
    val repositoryDisplayName: String,
    val state: String,
    val runtimeState: String,
) {
    val sessionId
        get() = activeSessionId ?: initialSessionId
}

@Serializable
data class RemoteEnvironment(
    val environmentId: String,
    val label: String? = null,
    val providerKind: String,
    val wireProtocolVersion: Int? = null,
    val capabilities: JsonElement = JsonNull,
) {
    val canonicalAccount: Boolean
        get() =
            wireProtocolVersion == 1 &&
                capabilities.array("features").any { it == "zuse-rpc-json-v1".json() }
}

@Serializable
data class Branch(
    val name: String,
    val current: Boolean,
    val remote: String? = null,
    val kind: String,
)

@Serializable
data class PendingMessage(
    val id: String,
    val messageId: String,
    val sessionId: String,
    val text: String,
    val attempted: Boolean = false,
    val attachments: List<JsonElement> = emptyList(),
) {
    val payload
        get() =
            obj(
                "sessionId" to sessionId.json(),
                "commandId" to id.json(),
                "clientMessageId" to messageId.json(),
                "input" to
                    obj(
                        "text" to text.json(),
                        "attachments" to JsonArray(attachments),
                        "fileRefs" to JsonArray(emptyList()),
                        "skillRefs" to JsonArray(emptyList()),
                        "annotations" to JsonArray(emptyList()),
                    ),
            )
}

@Serializable
data class SessionMetadata(
    val draftRevision: Long = 0,
    val formRevision: Long = 0,
    val draft: String = "",
    val pending: List<PendingMessage> = emptyList(),
    val attachments: List<JsonElement> = emptyList(),
    val responses: Map<String, String> = emptyMap(),
    val uiDrafts: Map<String, Map<String, JsonElement>> = emptyMap(),
)

data class SessionInputs(
    val draftRevision: Long,
    val draft: String,
    val formRevision: Long,
    val uiDrafts: Map<String, Map<String, JsonElement>>,
)

/**
 * Independent revisions stop late writes from reviving submitted text or replacing newer fields.
 */
fun SessionMetadata.withInputs(inputs: SessionInputs): SessionMetadata {
    var updated = this
    if (inputs.draftRevision > draftRevision)
        updated = updated.copy(draftRevision = inputs.draftRevision, draft = inputs.draft)
    if (inputs.formRevision > formRevision)
        updated = updated.copy(formRevision = inputs.formRevision, uiDrafts = inputs.uiDrafts)
    return updated.copy(uiDrafts = updated.uiDrafts - updated.responses.keys)
}

data class SessionArchive(
    val metadata: SessionMetadata = SessionMetadata(),
    val messages: List<Message> = emptyList(),
) {
    val draft
        get() = metadata.draft

    val pending
        get() = metadata.pending

    val attachments
        get() = metadata.attachments

    val responses
        get() = metadata.responses

    val uiDrafts
        get() = metadata.uiDrafts
}

data class DecodedMetadata(val metadata: SessionMetadata, val legacyMessages: List<Message>?)

fun decodeSessionMetadata(encoded: String): DecodedMetadata {
    val value = WireJson.parseToJsonElement(encoded)
    val history = value.field("messages")
    require(history == JsonNull || history is JsonArray) { "Saved chat history is unreadable." }
    val metadata: SessionMetadata = value.decode()
    val legacyRevision = value.long("inputRevision") ?: 0
    return DecodedMetadata(
        metadata.copy(
            draftRevision = value.long("draftRevision") ?: legacyRevision,
            formRevision = value.long("formRevision") ?: legacyRevision,
        ),
        (history as? JsonArray)?.decode(),
    )
}

class ClientException(message: String) : Exception(message)

class HttpRequestFailure(val status: Int, message: String) : Exception(message)

/** A definite server response, distinct from an unknown transport outcome. */
class RpcRequestFailure(val tag: String?, message: String) : Exception(message)
