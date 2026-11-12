package sh.zuse.core

import kotlinx.coroutines.async
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.*

/** Canonical engine projections. One-shot watches cancel immediately after their first value. */
class CanonicalWorkspace(private val rpc: RpcClient) {
    private suspend fun snapshot(method: String): List<JsonElement> =
        withTimeout(30_000) { rpc.stream(method).first().jsonArray.toList() }

    suspend fun projects(): List<Project> =
        snapshot("WatchSpaces").map { space ->
            val path = space.text("path").orEmpty()
            Project(
                requireNotNull(space.text("id")),
                space.text("name") ?: path.trimEnd('/').substringAfterLast('/').ifEmpty { path },
            )
        } + Project(PROJECTLESS, ChatList.NO_PROJECT)

    suspend fun sessions(projectId: String): List<Session> =
        snapshot("WatchChats")
            .filter {
                !it.flag("archived") &&
                    it.text("parentChatId") == null &&
                    (it.text("spaceId") ?: PROJECTLESS) == projectId
            }
            .map(::session)

    suspend fun allSessions(): List<Session> =
        snapshot("WatchChats")
            .filter { !it.flag("archived") && it.text("parentChatId") == null }
            .map(::session)

    suspend fun session(id: String): Session =
        session(snapshot("WatchChats").first { it.text("id") == id })

    /** Whether a chat with this id exists on the host, archived or not. */
    suspend fun exists(id: String): Boolean = snapshot("WatchChats").any { it.text("id") == id }

    /** Live run status per chat id; hosts without the runtime stream report none. */
    suspend fun runtimeStatuses(nowMillis: Long = System.currentTimeMillis()): Map<String, String> {
        val rows =
            try {
                withTimeout(5_000) { rpc.stream("WatchSessions").first().jsonArray.toList() }
            } catch (_: kotlinx.coroutines.TimeoutCancellationException) {
                emptyList()
            } catch (cancelled: kotlinx.coroutines.CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                emptyList()
            }
        return rows
            .mapNotNull { row ->
                val id = row.text("chatId") ?: return@mapNotNull null
                CanonicalRuntime.status(row, id, nowMillis)?.let { id to it }
            }
            .toMap()
    }

    suspend fun archivedSessions(): List<Session> =
        snapshot("WatchChats")
            .filter { it.flag("archived") && it.text("parentChatId") == null }
            .map(::session)

    suspend fun rename(id: String, title: String) {
        require(title.isNotBlank() && title.length <= 500)
        rpc.call(
            "Mutate",
            obj("op" to "renameChat".json(), "chatId" to id.json(), "title" to title.trim().json()),
        )
    }

    suspend fun setArchived(id: String, archived: Boolean) {
        rpc.call(
            "Mutate",
            obj(
                "op" to "setChatArchived".json(),
                "chatId" to id.json(),
                "archived" to archived.json(),
            ),
        )
    }

    private fun session(chat: JsonElement): Session {
        val config = chat.field("config")
        return Session(
            requireNotNull(chat.text("id")),
            chat.text("spaceId") ?: PROJECTLESS,
            chat.text("title") ?: "New chat",
            config.text("harness") ?: "",
            config.text("model") ?: "",
            preview = chat.text("lastMessagePreview")?.takeIf { it.isNotBlank() },
            activityAt = chat.text("lastMessageAt") ?: chat.text("createdAt"),
            branch = chat.text("branch")?.takeIf { it.isNotBlank() },
        )
    }

    suspend fun catalog(): JsonElement {
        val providers = linkedMapOf<String, JsonElement>()
        val harnesses = rpc.call("ListHarnesses").jsonArray
        kotlinx.coroutines.coroutineScope {
            val requests =
                harnesses
                    .filter {
                        it.field("installed") != false.json() && it.field("enabled") != false.json()
                    }
                    .map { harness ->
                        val id = requireNotNull(harness.text("id"))
                        id to async { rpc.call("ListModels", obj("harness" to id.json())) }
                    }
            for ((id, request) in requests) {
                val models =
                    request.await().jsonArray.map { model ->
                        JsonObject(model.jsonObject + ("available" to true.json()))
                    }
                providers[id] = obj("models" to JsonArray(models))
            }
        }
        return obj("providers" to JsonObject(providers))
    }

    suspend fun diff(chatId: String, scope: String): JsonElement {
        require(scope in listOf("staged", "unstaged", "branch"))
        val chat = snapshot("WatchChats").first { it.text("id") == chatId }
        return rpc.call(
            "GetCheckoutDiff",
            obj("chatId" to chatId.json(), "cwd" to chat.field("cwd"), "mode" to scope.json()),
        )
    }

    suspend fun branches(projectId: String): List<Branch> {
        if (projectId == PROJECTLESS) return emptyList()
        val space = snapshot("WatchSpaces").first { it.text("id") == projectId }
        val refs = rpc.call("ListRefs", obj("repoPath" to space.field("path"))).jsonArray
        return refs.map { ref ->
            Branch(requireNotNull(ref.text("name")), ref.flag("current"), kind = "local")
        }
    }

    suspend fun create(request: JsonObject, branch: Branch?, fresh: Boolean): Session {
        val id = requireNotNull(request.text("operationId"))
        val projectId = requireNotNull(request.text("projectId"))
        snapshot("WatchChats")
            .firstOrNull { it.text("id") == id }
            ?.let {
                require((it.text("spaceId") ?: PROJECTLESS) == projectId) {
                    "The saved creation belongs to another project."
                }
                return session(it)
            }
        val space =
            if (projectId == PROJECTLESS) null
            else snapshot("WatchSpaces").first { it.text("id") == projectId }
        val host = space?.text("deviceId") ?: rpc.call("EngineInfo").text("deviceId")
        var cwd = space?.text("path")
        var selectedBranch = branch?.name
        if (fresh || (branch != null && !branch.current)) {
            require(space != null) { "Choose a project for an isolated checkout." }
            val outcome =
                if (branch != null)
                    rpc.call(
                            "ResolveCreationSource",
                            obj(
                                "repoPath" to space.field("path"),
                                "spaceId" to space.field("id"),
                                "requestId" to id.json(),
                                "source" to
                                    obj(
                                        "kind" to "branch".json(),
                                        "reference" to branch.name.json(),
                                    ),
                            ),
                        )
                        .field("outcome")
                else
                    rpc.call(
                        "CreateWorktree",
                        obj(
                            "repoPath" to space.field("path"),
                            "spaceId" to space.field("id"),
                            "branch" to "mobile-$id".json(),
                        ),
                    )
            cwd =
                outcome.field("worktree").text("path")
                    ?: throw ClientException("The host did not return a checkout path.")
            selectedBranch = outcome.field("worktree").text("branch")
        }
        rpc.call(
            "Mutate",
            obj(
                "op" to "createChat".json(),
                "chatId" to id.json(),
                "spaceId" to space?.field("id"),
                "deviceId" to host?.json(),
                "cwd" to cwd?.json(),
                "branch" to selectedBranch?.json(),
                "config" to
                    obj(
                        "harness" to request.field("providerId"),
                        "model" to request.field("model"),
                        "reasoning" to JsonNull,
                        "modelOptions" to obj(),
                        "sandbox" to "workspace-write".json(),
                    ),
            ),
        )
        return session(id)
    }

    suspend fun answer(
        chatId: String,
        requestId: String,
        questions: List<JsonElement>,
        answers: JsonArray,
    ) {
        val encoded =
            answers.map { answer ->
                val index = requireNotNull(answer.long("questionIndex")).toInt()
                val question =
                    questions.getOrNull(index)
                        ?: throw ClientException("The question changed. Refresh before answering.")
                val labels =
                    answer.array("selected").map { selected ->
                        val option = selected.jsonPrimitive.int
                        question.array("options").getOrNull(option)
                            ?: throw ClientException("The answer option changed.")
                    } + listOfNotNull(answer.text("other")?.takeIf { it.isNotBlank() }?.json())
                obj("questionId" to question.field("id"), "labels" to JsonArray(labels))
            }
        rpc.call(
            "QueueCommand",
            obj(
                "chatId" to chatId.json(),
                "command" to
                    obj(
                        "kind" to "respondInput".json(),
                        "requestId" to requestId.json(),
                        "answers" to JsonArray(encoded),
                    ),
            ),
        )
    }

    suspend fun changeModel(chatId: String, harness: String, model: String) {
        val chat = snapshot("WatchChats").first { it.text("id") == chatId }
        val previous = chat.field("config")
        val config =
            JsonObject(
                (previous as? JsonObject).orEmpty() +
                    mapOf("harness" to harness.json(), "model" to model.json())
            )
        rpc.call(
            "Mutate",
            obj(
                "op" to "setChatConfig".json(),
                "chatId" to chatId.json(),
                "expectedConfig" to obj("deviceId" to chat.field("deviceId"), "config" to previous),
                "config" to config,
            ),
        )
    }

    suspend fun download(path: String, output: java.io.OutputStream): JsonElement =
        kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
            var offset = 0L
            var metadata: JsonElement
            do {
                kotlinx.coroutines.currentCoroutineContext().ensureActive()
                metadata =
                    rpc.call(
                        "ReadAttachmentChunk",
                        obj("path" to path.json(), "offset" to offset.json()),
                    )
                val bytes =
                    java.util.Base64.getDecoder().decode(requireNotNull(metadata.text("data")))
                val next = requireNotNull(metadata.long("nextOffset"))
                require(
                    next == offset + bytes.size &&
                        next <= 32L * 1024 * 1024 &&
                        (next > offset || metadata.flag("done"))
                ) {
                    "Invalid attachment chunk."
                }
                output.write(bytes)
                offset = next
            } while (!metadata.flag("done"))
            metadata
        }

    suspend fun upload(file: java.io.File, name: String, mime: String): JsonElement =
        kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
            require(file.length() <= 32L * 1024 * 1024) {
                "The host supports attachments up to 32 MB."
            }
            val id = java.util.UUID.randomUUID().toString()
            file.inputStream().use { input ->
                val buffer = ByteArray(45 * 1024)
                var sequence = 0L
                do {
                    kotlinx.coroutines.currentCoroutineContext().ensureActive()
                    val size = input.read(buffer)
                    if (size < 0 && sequence > 0) break
                    rpc.call(
                        "UploadChunk",
                        obj(
                            "uploadId" to id.json(),
                            "seq" to sequence.json(),
                            "data" to
                                java.util.Base64.getEncoder()
                                    .encodeToString(buffer.copyOf(size.coerceAtLeast(0)))
                                    .json(),
                        ),
                    )
                    sequence++
                } while (size >= 0)
            }
            val result =
                rpc.call("UploadCommit", obj("uploadId" to id.json(), "fileName" to name.json()))
            obj(
                "id" to id.json(),
                "path" to requireNotNull(result.text("path")).json(),
                "originalName" to name.json(),
                "mimeType" to mime.json(),
            )
        }

    suspend fun send(item: PendingMessage): JsonElement {
        val paths =
            item.attachments.map {
                requireNotNull(it.text("path")) {
                    "Upload this attachment to the connected host again."
                }
            }
        val prompt =
            if (paths.isEmpty()) item.text
            else
                (item.text.ifBlank { "Please review the attached files." } +
                    "\n\nAttached images (local files — open them to view):\n" +
                    paths.joinToString("\n") { "- $it" })
        // Checks before QueueCommand fail as IllegalArgumentException: the message was never sent.
        val chat =
            requireNotNull(snapshot("WatchChats").firstOrNull { it.text("id") == item.sessionId }) {
                "This chat is no longer on the host."
            }
        val config = chat.field("config")
        val harness = requireNotNull(config.text("harness")) { "Choose an agent for this chat." }
        val request =
            obj(
                "prompt" to prompt.json(),
                "attachments" to JsonArray(paths.map { it.json() }),
                "harness" to harness.json(),
                "model" to config.field("model"),
                "reasoning" to config.field("reasoning"),
                "modelOptions" to (config.field("modelOptions") as? JsonObject ?: obj()),
                "cwd" to
                    requireNotNull(chat.text("cwd")) {
                            "The host has not resolved this chat's folder."
                        }
                        .json(),
                "sandbox" to (config.text("sandbox") ?: "workspace-write").json(),
                "autoApprove" to false.json(),
                "resume" to chat.field("harnessSessionId"),
            )
        return rpc.call(
            "QueueCommand",
            obj(
                "chatId" to item.sessionId.json(),
                "command" to
                    obj(
                        "kind" to "run".json(),
                        "messageId" to item.messageId.json(),
                        "request" to request,
                    ),
            ),
        )
    }

    suspend fun stop(chatId: String): JsonElement =
        rpc.call(
            "QueueCommand",
            obj("chatId" to chatId.json(), "command" to obj("kind" to "interrupt".json())),
        )

    companion object {
        const val PROJECTLESS = "__projectless__"
    }
}
