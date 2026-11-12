package sh.zuse.android

import android.content.Intent
import android.net.Uri
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.mockwebserver.*
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import sh.zuse.core.*

@RunWith(AndroidJUnit4::class)
class NativeFlowTests {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private fun launch(uri: String) {
        compose.activityRule.scenario.onActivity { activity ->
            activity.startActivity(
                Intent(activity, MainActivity::class.java)
                    .setData(Uri.parse(uri))
                    .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
            )
        }
    }

    @Test
    fun backFromComputersReturnsToSettings() {
        compose.onNodeWithContentDescription("Settings").performClick()
        compose.onNodeWithText("Computers").performClick()
        compose.onNodeWithText("Add manually").assertExists()
        check(
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()
                .uiAutomation
                .performGlobalAction(
                    android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK
                )
        )
        compose.waitUntil(5000) {
            compose.onAllNodesWithText("Zuse Cloud").fetchSemanticsNodes().isNotEmpty()
        }
        compose.onNodeWithText("Add manually").assertDoesNotExist()
        compose.onNodeWithText("Zuse Cloud").assertExists()
    }

    @Test
    fun encryptedLegacyArchiveMigratesWithoutRewritingHistoryDuringDraftEdits() =
        kotlinx.coroutines.runBlocking {
            val vault = (compose.activity.application as ZuseApplication).vault
            val identity = "migration-test:${java.util.UUID.randomUUID()}"
            val message =
                Message(
                    "m",
                    "s",
                    "assistant",
                    obj("_tag" to "assistant".json(), "text" to "Retained history".json()),
                )
            vault.write(
                "archive:$identity",
                obj(
                        "draft" to "Keep me".json(),
                        "messages" to WireJson.encodeToJsonElement(listOf(message)),
                    )
                    .toString(),
            )
            assertEquals(listOf(message), vault.archive(identity).messages)
            assertEquals("Keep me", vault.archive(identity).draft)
            val history = vault.read("history:$identity")
            vault.updateMetadata(identity) { it.copy(draft = "Edited") }
            assertEquals(history, vault.read("history:$identity"))
            assertEquals("Edited", vault.archive(identity).draft)
            assertEquals(
                JsonNull,
                WireJson.parseToJsonElement(vault.read("archive:$identity")!!).field("messages"),
            )
            val parameters = obj("projectId" to "project".json())
            val original =
                vault.prepareChatIntent("$identity:create", parameters, "Original prompt")
            val edited = vault.prepareChatIntent("$identity:create", parameters, "Edited prompt")
            assertEquals(original.first, edited.first)
            assertEquals("Original prompt", edited.second)
            assertEquals(JsonNull, edited.first.field("prompt"))
            vault.write("$identity:create", null)
            vault.write("history:$identity", null)
            vault.write("archive:$identity", null)
        }

    @Test
    fun canonicalVisualParserRunsInAnIsolatedEngine() =
        kotlinx.coroutines.runBlocking {
            org.junit.Assume.assumeTrue(androidx.javascriptengine.JavaScriptSandbox.isSupported())
            val app = compose.activity.application as ZuseApplication
            val tree = app.uiParser.parse("root = Card([Text(\"Native visual\")], \"Status\")")
            assertEquals("Card", tree.text("typeName"))
            assertEquals("Status", tree.field("props").text("title"))
            var rejected = false
            try {
                app.uiParser.parse("root = Text(globalThis.secret)")
            } catch (_: Exception) {
                rejected = true
            }
            assertTrue(rejected)
            app.uiParser.close()
        }

    @Test
    fun largeTranscriptAndComposerAreReachable() {
        launch("zuse://preview?large=true")
        compose.waitUntil(30_000) {
            compose
                .onAllNodesWithContentDescription("Chat transcript")
                .fetchSemanticsNodes()
                .isNotEmpty()
        }
        compose.onNodeWithContentDescription("Chat transcript").performScrollToIndex(0)
        compose.onNodeWithContentDescription("Chat transcript").performScrollToIndex(9999)
        compose
            .onNodeWithContentDescription("Ask Zuse")
            .assertIsDisplayed()
            .performTextInput("Keep the composer native")
        compose.onNodeWithContentDescription("Send message").assertIsEnabled()
    }

    @Test
    fun runtimeConnectionStreamsAndSendsThroughTheNativeComposer() {
        val server = MockWebServer()
        val sent = LinkedBlockingQueue<JsonElement>()
        val session = Session("s", "p", "Local fixture chat", "claude", "test-model")
        val message =
            Message(
                "m",
                "s",
                "assistant",
                obj(
                    "_tag" to "text".json(),
                    "text" to "Connected through the real WebSocket transport.".json(),
                ),
            )
        server.enqueue(
            MockResponse()
                .withWebSocketUpgrade(
                    object : WebSocketListener() {
                        override fun onMessage(socket: WebSocket, text: String) {
                            val request = WireJson.parseToJsonElement(text)
                            if (request.text("_tag") != "Request") return
                            val method = request.text("tag")
                            val result: JsonElement =
                                when (method) {
                                    "connect.handshake" -> obj("protocolVersion" to 7.json())
                                    "workspace.list" ->
                                        WireJson.encodeToJsonElement(
                                            listOf(Project("p", "Fixture project"))
                                        )
                                    "session.list" -> WireJson.encodeToJsonElement(listOf(session))
                                    "model.catalog" ->
                                        obj(
                                            "providers" to
                                                obj(
                                                    "claude" to
                                                        obj(
                                                            "models" to
                                                                JsonArray(
                                                                    listOf(
                                                                        obj(
                                                                            "id" to
                                                                                "test-model".json(),
                                                                            "label" to
                                                                                "Test model".json(),
                                                                            "available" to
                                                                                true.json(),
                                                                        )
                                                                    )
                                                                )
                                                        )
                                                )
                                        )
                                    "messages.send" -> {
                                        sent.add(request.field("payload"))
                                        JsonNull
                                    }
                                    "session.events" -> {
                                        val frames =
                                            listOf(
                                                obj(
                                                    "kind" to "snapshot".json(),
                                                    "sessionId" to "s".json(),
                                                    "throughVersion" to 0.json(),
                                                    "projection" to
                                                        obj(
                                                            "messages" to
                                                                WireJson.encodeToJsonElement(
                                                                    listOf(message)
                                                                )
                                                        ),
                                                ),
                                                obj(
                                                    "kind" to "synchronized".json(),
                                                    "sessionId" to "s".json(),
                                                    "throughVersion" to 0.json(),
                                                ),
                                            )
                                        socket.send(
                                            obj(
                                                    "_tag" to "Chunk".json(),
                                                    "requestId" to request.field("id"),
                                                    "values" to JsonArray(frames),
                                                )
                                                .toString()
                                        )
                                        return
                                    }
                                    else -> JsonNull
                                }
                            socket.send(
                                obj(
                                        "_tag" to "Exit".json(),
                                        "requestId" to request.field("id"),
                                        "exit" to obj("_tag" to "Success".json(), "value" to result),
                                    )
                                    .toString()
                            )
                        }
                    }
                )
        )
        server.start()
        try {
            val address = server.url("/rpc").toString().replace("http://", "ws://")
            launch("zuse://fixture?address=${Uri.encode(address)}")
            compose.waitUntil(30_000) {
                compose.onAllNodesWithText("Local fixture chat").fetchSemanticsNodes().isNotEmpty()
            }
            compose.onNodeWithText("Local fixture chat").performClick()
            compose.onNodeWithContentDescription("Ask Zuse").performTextInput("Hello from Kotlin")
            compose.onNodeWithContentDescription("Send message").performClick()
            val payload = sent.poll(15, TimeUnit.SECONDS)
            assertNotNull(payload)
            assertEquals("Hello from Kotlin", payload!!.field("input").text("text"))
            assertFalse(payload.text("commandId").isNullOrEmpty())
            assertFalse(payload.text("clientMessageId").isNullOrEmpty())
        } finally {
            server.shutdown()
        }
    }

    @Test
    fun canonicalRustContractStreamsAndSendsThroughComposer() {
        val sent = LinkedBlockingQueue<JsonElement>()
        val server = MockWebServer()
        val chat =
            obj(
                "id" to "canonical-chat".json(),
                "spaceId" to "p".json(),
                "title" to "Rust remote chat".json(),
                "cwd" to "/host/project".json(),
                "config" to
                    obj(
                        "harness" to "codex".json(),
                        "model" to "test-model".json(),
                        "sandbox" to "workspace-write".json(),
                    ),
            )
        val currentChat = java.util.concurrent.atomic.AtomicReference(chat)
        server.enqueue(
            MockResponse()
                .withWebSocketUpgrade(
                    object : WebSocketListener() {
                        override fun onMessage(socket: WebSocket, text: String) {
                            val frame = WireJson.parseToJsonElement(text)
                            val id = frame.field("id")
                            val method = frame.text("method") ?: return
                            val result =
                                when (method) {
                                    "EngineInfo" -> obj("deviceId" to "host".json())
                                    "WatchSessions" -> JsonArray(emptyList())
                                    "WatchSpaces" ->
                                        JsonArray(
                                            listOf(
                                                obj(
                                                    "id" to "p".json(),
                                                    "name" to "Fixture project".json(),
                                                    "path" to "/host/project".json(),
                                                )
                                            )
                                        )
                                    "WatchChats" -> JsonArray(listOf(currentChat.get()))
                                    "Mutate" -> {
                                        val params = frame.field("params")
                                        val key =
                                            if (params.text("op") == "renameChat") "title"
                                            else "archived"
                                        currentChat.updateAndGet {
                                            JsonObject(it + (key to params.field(key)))
                                        }
                                        obj("ok" to true.json())
                                    }
                                    "ListHarnesses" ->
                                        JsonArray(
                                            listOf(
                                                obj(
                                                    "id" to "codex".json(),
                                                    "installed" to true.json(),
                                                    "enabled" to true.json(),
                                                )
                                            )
                                        )
                                    "ListModels" ->
                                        JsonArray(
                                            listOf(
                                                obj(
                                                    "id" to "test-model".json(),
                                                    "label" to "Test model".json(),
                                                )
                                            )
                                        )
                                    "WatchDocMessages" ->
                                        obj(
                                            "reset" to
                                                JsonArray(
                                                    listOf(
                                                        obj(
                                                            "id" to "m".json(),
                                                            "role" to "assistant".json(),
                                                            "parts" to
                                                                JsonArray(
                                                                    listOf(
                                                                        obj(
                                                                            "id" to "text".json(),
                                                                            "kind" to "text".json(),
                                                                            "text" to
                                                                                "Rust transcript connected"
                                                                                    .json(),
                                                                        )
                                                                    )
                                                                ),
                                                        )
                                                    )
                                                )
                                        )
                                    "QueueCommand" -> {
                                        sent.add(frame.field("params"))
                                        obj("commandId" to "accepted".json())
                                    }
                                    else -> {
                                        socket.send(
                                            obj(
                                                    "id" to id,
                                                    "err" to "Unexpected method $method".json(),
                                                )
                                                .toString()
                                        )
                                        return
                                    }
                                }
                            socket.send(
                                obj(
                                        "id" to id,
                                        (if (method.startsWith("Watch")) "item" else "ok") to result,
                                    )
                                    .toString()
                            )
                        }
                    }
                )
        )
        server.start()
        try {
            val address = server.url("/api/rpc").toString().replace("http://", "ws://")
            launch("zuse://fixture?address=${Uri.encode(address)}")
            compose.waitUntil(30_000) {
                compose.onAllNodesWithText("Rust remote chat").fetchSemanticsNodes().isNotEmpty()
            }
            compose.onNodeWithText("Rust remote chat").performClick()
            compose.waitUntil(15_000) {
                fun containsText(view: android.view.View): Boolean =
                    (view is android.widget.TextView &&
                        view.text.toString().contains("Rust transcript connected")) ||
                        (view is android.view.ViewGroup &&
                            (0 until view.childCount).any { containsText(view.getChildAt(it)) })
                var rendered = false
                compose.runOnUiThread { rendered = containsText(compose.activity.window.decorView) }
                rendered
            }
            compose.onNodeWithContentDescription("Ask Zuse").performTextInput("Hello Rust host")
            compose.onNodeWithContentDescription("Send message").performClick()
            val command = sent.poll(15, TimeUnit.SECONDS)
            assertNotNull(command)
            assertEquals("canonical-chat", command!!.text("chatId"))
            assertEquals(
                "Hello Rust host",
                command.field("command").field("request").text("prompt"),
            )
            compose.onNodeWithContentDescription("Chat actions").performClick()
            compose.onNodeWithText("Rename chat").performClick()
            compose.onNodeWithText("Chat title").performTextReplacement("Renamed remote chat")
            compose.onNodeWithText("Save name").performClick()
            compose.waitUntil(5000) {
                compose.onAllNodesWithText("Renamed remote chat").fetchSemanticsNodes().isNotEmpty()
            }
            assertEquals("Renamed remote chat", currentChat.get().text("title"))
            compose.onNodeWithContentDescription("Chat actions").performClick()
            compose.onNodeWithText("Archive chat").performClick()
            compose.onNodeWithText("Archive").performClick()
            compose.waitUntil(5000) { currentChat.get().flag("archived") }
            compose.onNodeWithContentDescription("Settings").performClick()
            compose.onNodeWithText("Archived chats").performClick()
            compose.waitUntil(5000) {
                compose.onAllNodesWithText("Restore").fetchSemanticsNodes().isNotEmpty()
            }
            compose.onNodeWithText("Restore").performClick()
            compose.waitUntil(5000) {
                !currentChat.get().flag("archived") &&
                    compose
                        .onAllNodesWithText("No archived chats on this computer.")
                        .fetchSemanticsNodes()
                        .isNotEmpty()
            }
        } finally {
            server.shutdown()
        }
    }

    @Test
    fun newChatSendsTheFirstMessageWithDefaultsAndARejectionIsNotUncertain() {
        val server = MockWebServer()
        val mutations = LinkedBlockingQueue<JsonElement>()
        val commands = LinkedBlockingQueue<JsonElement>()
        val isolated = LinkedBlockingQueue<String>()
        val chats = java.util.concurrent.atomic.AtomicReference(emptyList<JsonElement>())
        val rejectNext = java.util.concurrent.atomic.AtomicBoolean(true)
        server.enqueue(
            MockResponse()
                .withWebSocketUpgrade(
                    object : WebSocketListener() {
                        override fun onMessage(socket: WebSocket, text: String) {
                            val frame = WireJson.parseToJsonElement(text)
                            val id = frame.field("id")
                            val method = frame.text("method") ?: return
                            val params = frame.field("params")
                            fun reply(result: JsonElement) =
                                socket.send(
                                    obj(
                                            "id" to id,
                                            (if (method.startsWith("Watch")) "item" else "ok") to
                                                result,
                                        )
                                        .toString()
                                )
                            fun reject(message: String) =
                                socket.send(obj("id" to id, "err" to message.json()).toString())
                            when (method) {
                                "EngineInfo" -> reply(obj("deviceId" to "host".json()))
                                "WatchSpaces",
                                "WatchSessions" -> reply(JsonArray(emptyList()))
                                "WatchChats" -> reply(JsonArray(chats.get()))
                                "ListHarnesses" ->
                                    reply(
                                        JsonArray(
                                            listOf(
                                                obj(
                                                    "id" to "claude".json(),
                                                    "installed" to true.json(),
                                                    "enabled" to true.json(),
                                                )
                                            )
                                        )
                                    )
                                "ListModels" ->
                                    reply(
                                        JsonArray(
                                            listOf(
                                                obj(
                                                    "id" to "test-model".json(),
                                                    "label" to "Test model".json(),
                                                )
                                            )
                                        )
                                    )
                                "CreateWorktree",
                                "ResolveCreationSource" -> {
                                    isolated.add(method)
                                    reject("No project to check out")
                                }
                                "Mutate" -> {
                                    mutations.add(params)
                                    if (rejectNext.getAndSet(false)) reject("Rejected by host")
                                    else {
                                        chats.set(
                                            listOf(
                                                obj(
                                                    "id" to params.field("chatId"),
                                                    "title" to "New chat".json(),
                                                    "cwd" to "/home/fixture".json(),
                                                    "config" to params.field("config"),
                                                )
                                            )
                                        )
                                        reply(obj("ok" to true.json()))
                                    }
                                }
                                "WatchDocMessages" -> reply(obj("reset" to JsonArray(emptyList())))
                                "QueueCommand" -> {
                                    commands.add(params)
                                    reply(obj("commandId" to "accepted".json()))
                                }
                                else -> reject("Unexpected method $method")
                            }
                        }
                    }
                )
        )
        server.start()
        try {
            val address = server.url("/api/rpc").toString().replace("http://", "ws://")
            launch("zuse://fixture?address=${Uri.encode(address)}")
            compose.waitUntil(30_000) {
                compose.onAllNodesWithText("New chat").fetchSemanticsNodes().isNotEmpty()
            }
            compose.onNodeWithText("New chat").performClick()
            compose.waitUntil(15_000) {
                compose.onAllNodesWithText(ChatList.NO_PROJECT).fetchSemanticsNodes().isNotEmpty()
            }
            // Without a project there is no mode to pick: the chat works locally.
            compose.onNodeWithText("Mode").assertDoesNotExist()
            compose
                .onNodeWithContentDescription("New chat message")
                .performTextInput("Hello new chat")
            compose.onNodeWithContentDescription("Send message").performClick()
            compose.waitUntil(15_000) {
                compose.onAllNodesWithText("Rejected by host").fetchSemanticsNodes().isNotEmpty()
            }
            compose.onNodeWithText("uncertain", substring = true).assertDoesNotExist()
            val rejected = mutations.poll(5, TimeUnit.SECONDS)!!
            // A rejected request is not uncertain: sending again simply works.
            compose.onNodeWithContentDescription("Send message").performClick()
            val created = mutations.poll(15, TimeUnit.SECONDS)
            assertNotNull(created)
            assertEquals("createChat", created!!.text("op"))
            assertEquals(JsonNull, created.field("spaceId"))
            assertEquals(JsonNull, created.field("cwd"))
            assertNotEquals(rejected.text("chatId"), created.text("chatId"))
            val command = commands.poll(15, TimeUnit.SECONDS)
            assertNotNull(command)
            assertEquals(created.text("chatId"), command!!.text("chatId"))
            assertEquals("Hello new chat", command.field("command").field("request").text("prompt"))
            assertTrue(isolated.isEmpty())
            compose.waitUntil(15_000) {
                compose
                    .onAllNodesWithContentDescription("New chat message")
                    .fetchSemanticsNodes()
                    .isEmpty()
            }
            compose.onNodeWithText("Rejected by host").assertDoesNotExist()
            compose.onNodeWithContentDescription("Ask Zuse").assertExists()
        } finally {
            server.shutdown()
        }
    }
}
