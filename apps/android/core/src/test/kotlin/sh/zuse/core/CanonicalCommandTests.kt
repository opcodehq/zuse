package sh.zuse.core

import kotlin.test.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.mockwebserver.*

class CanonicalCommandTests {
    @Test
    fun `send uses host configuration and stable message identity without auto approval`() =
        runBlocking {
            val commands = java.util.concurrent.LinkedBlockingQueue<JsonElement>()
            val server = MockWebServer()
            server.enqueue(
                MockResponse()
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onMessage(socket: WebSocket, text: String) {
                                val frame = WireJson.parseToJsonElement(text)
                                val id = frame.field("id")
                                when (frame.text("method")) {
                                    "EngineInfo" ->
                                        socket.send(obj("id" to id, "ok" to obj()).toString())
                                    "WatchChats" ->
                                        socket.send(
                                            obj(
                                                    "id" to id,
                                                    "item" to
                                                        JsonArray(
                                                            listOf(
                                                                obj(
                                                                    "id" to "chat".json(),
                                                                    "cwd" to "/host/project".json(),
                                                                    "config" to
                                                                        obj(
                                                                            "harness" to
                                                                                "codex".json(),
                                                                            "model" to
                                                                                "host-model".json(),
                                                                            "sandbox" to
                                                                                "read-only".json(),
                                                                        ),
                                                                )
                                                            )
                                                        ),
                                                )
                                                .toString()
                                        )
                                    "QueueCommand" -> {
                                        commands.add(frame.field("params"))
                                        socket.send(
                                            obj(
                                                    "id" to id,
                                                    "ok" to obj("commandId" to "queued".json()),
                                                )
                                                .toString()
                                        )
                                    }
                                }
                            }
                        }
                    )
            )
            server.start()
            val client = RpcClient(server.url("/api/rpc").toString().replace("http:", "ws:"))
            try {
                client.connect()
                val workspace = CanonicalWorkspace(client)
                workspace.send(PendingMessage("operation", "stable-message", "chat", "hello"))
                val command =
                    commands.poll(5, java.util.concurrent.TimeUnit.SECONDS)!!.field("command")
                assertEquals("stable-message", command.text("messageId"))
                assertEquals("run", command.text("kind"))
                val request = command.field("request")
                assertEquals("/host/project", request.text("cwd"))
                assertEquals("read-only", request.text("sandbox"))
                assertEquals("host-model", request.text("model"))
                assertFalse(request.flag("autoApprove"))
                workspace.stop("chat")
                assertEquals(
                    "interrupt",
                    commands
                        .poll(5, java.util.concurrent.TimeUnit.SECONDS)!!
                        .field("command")
                        .text("kind"),
                )
                workspace.answer(
                    "chat",
                    "request",
                    listOf(
                        obj(
                            "id" to "q".json(),
                            "options" to JsonArray(listOf("Allow".json(), "Deny".json())),
                        )
                    ),
                    JsonArray(
                        listOf(
                            obj(
                                "questionIndex" to 0.json(),
                                "selected" to JsonArray(listOf(1.json())),
                            )
                        )
                    ),
                )
                val response =
                    commands.poll(5, java.util.concurrent.TimeUnit.SECONDS)!!.field("command")
                assertEquals("respondInput", response.text("kind"))
                assertEquals("request", response.text("requestId"))
                assertEquals("q", response.array("answers").single().text("questionId"))
                assertEquals(
                    JsonArray(listOf("Deny".json())),
                    response.array("answers").single().field("labels"),
                )
            } finally {
                client.close()
                server.shutdown()
            }
        }
}
