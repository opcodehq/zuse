package sh.zuse.core

import kotlin.test.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.mockwebserver.*

class CanonicalRpcTests {
    @Test
    fun `canonical connection authenticates without leaking token into URL and handles null streams errors`() =
        runBlocking {
            val requests = java.util.concurrent.LinkedBlockingQueue<JsonObject>()
            val cancelled = java.util.concurrent.CountDownLatch(1)
            val server = MockWebServer()
            server.enqueue(
                MockResponse()
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onMessage(socket: WebSocket, text: String) {
                                val frame = WireJson.parseToJsonElement(text).jsonObject
                                requests.add(frame)
                                if (frame.flag("cancel")) cancelled.countDown()
                                val id = frame.getValue("id")
                                when (frame.text("method")) {
                                    "EngineInfo" ->
                                        socket.send(
                                            obj(
                                                    "id" to id,
                                                    "ok" to obj("deviceId" to "host".json()),
                                                )
                                                .toString()
                                        )
                                    "Null" ->
                                        socket.send(obj("id" to id, "ok" to JsonNull).toString())
                                    "WatchChats" ->
                                        socket.send(
                                            obj("id" to id, "item" to JsonArray(emptyList()))
                                                .toString()
                                        )
                                    "WatchSpaces" ->
                                        socket.send(
                                            obj(
                                                    "id" to id,
                                                    "item" to
                                                        JsonArray(
                                                            listOf(
                                                                obj(
                                                                    "id" to "project".json(),
                                                                    "path" to
                                                                        "/home/code/project".json(),
                                                                )
                                                            )
                                                        ),
                                                )
                                                .toString()
                                        )
                                    "ListHarnesses" ->
                                        socket.send(
                                            obj(
                                                    "id" to id,
                                                    "ok" to
                                                        JsonArray(
                                                            listOf(
                                                                obj(
                                                                    "id" to "codex".json(),
                                                                    "installed" to true.json(),
                                                                    "enabled" to true.json(),
                                                                )
                                                            )
                                                        ),
                                                )
                                                .toString()
                                        )
                                    "ListModels" ->
                                        socket.send(
                                            obj(
                                                    "id" to id,
                                                    "ok" to
                                                        JsonArray(
                                                            listOf(
                                                                obj(
                                                                    "id" to "model".json(),
                                                                    "label" to "Model".json(),
                                                                )
                                                            )
                                                        ),
                                                )
                                                .toString()
                                        )
                                    "Fail" ->
                                        socket.send(
                                            obj("id" to id, "err" to "denied".json()).toString()
                                        )
                                }
                            }
                        }
                    )
            )
            server.start()
            val client =
                RpcClient(
                    server.url("/api/rpc").toString().replace("http:", "ws:"),
                    token = "private-test-token",
                )
            try {
                client.connect()
                val upgrade = server.takeRequest()
                assertNull(upgrade.requestUrl!!.query)
                assertEquals("Bearer private-test-token", upgrade.getHeader("Authorization"))
                assertEquals(
                    server.url("/").toString().removeSuffix("/"),
                    upgrade.getHeader("Origin"),
                )
                val workspace = CanonicalWorkspace(client)
                assertEquals(Project("project", "project"), workspace.projects().first())
                assertEquals(CanonicalWorkspace.PROJECTLESS, workspace.projects().last().id)
                assertTrue(workspace.sessions("project").isEmpty())
                assertTrue(
                    workspace
                        .catalog()
                        .field("providers")
                        .field("codex")
                        .array("models")
                        .single()
                        .flag("available")
                )
                assertEquals(JsonNull, client.call("Null"))
                assertEquals(
                    JsonArray(emptyList()),
                    withTimeout(2000) { client.stream("WatchChats").first() },
                )
                assertFailsWith<RpcRequestFailure> { client.call("Fail") }
                assertTrue(cancelled.await(2, java.util.concurrent.TimeUnit.SECONDS))
                val frames = generateSequence { requests.poll() }.toList()
                assertTrue(frames.all { it["id"]!!.jsonPrimitive.longOrNull != null })
                assertTrue(frames.any { it.flag("cancel") })
            } finally {
                client.close()
                server.shutdown()
            }
        }
}
