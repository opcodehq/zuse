package sh.zuse.core

import kotlin.test.*
import kotlinx.coroutines.*
import okhttp3.*
import okhttp3.mockwebserver.*
import org.junit.Test

class CloudGatewayTests {
    @Test
    fun `canonical gateway waits before RPC and runtime reset fails pending commands`() =
        runBlocking {
            val server = MockWebServer()
            val opened = CompletableDeferred<WebSocket>()
            val commands = CompletableDeferred<Unit>()
            val requests = java.util.concurrent.atomic.AtomicInteger()
            server.enqueue(
                MockResponse()
                    .setHeader("Sec-WebSocket-Protocol", CloudGateway.RUST)
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onOpen(webSocket: WebSocket, response: Response) {
                                webSocket.send(
                                    obj(
                                            "_zuseGateway" to "availability".json(),
                                            "state" to "pending".json(),
                                        )
                                        .toString()
                                )
                                opened.complete(webSocket)
                            }

                            override fun onMessage(webSocket: WebSocket, text: String) {
                                val frame = WireJson.parseToJsonElement(text)
                                requests.incrementAndGet()
                                if (frame.text("method") == "EngineInfo")
                                    webSocket.send(
                                        obj(
                                                "id" to frame.field("id"),
                                                "ok" to obj("deviceId" to "guest".json()),
                                            )
                                            .toString()
                                    )
                                else commands.complete(Unit)
                            }
                        }
                    )
            )
            server.start()
            val client =
                RpcClient(
                    server.url("/gateway").toString().replaceFirst("http://", "ws://"),
                    listOf(CloudGateway.RUST, "ticket"),
                )
            try {
                val connecting = async { client.connect() }
                val socket = withTimeout(3000) { opened.await() }
                delay(150)
                assertEquals(0, requests.get())
                socket.send(
                    obj(
                            "_zuseGateway" to "availability".json(),
                            "state" to "available".json(),
                            "reset" to false.json(),
                        )
                        .toString()
                )
                connecting.await()
                val command = async { runCatching { client.call("QueueCommand") } }
                withTimeout(3000) { commands.await() }
                socket.send(
                    obj(
                            "_zuseGateway" to "availability".json(),
                            "state" to "available".json(),
                            "reset" to true.json(),
                        )
                        .toString()
                )
                assertTrue(command.await().isFailure)
                assertFails { client.call("QueueCommand") }
                assertNull(server.takeRequest().requestUrl!!.query)
            } finally {
                client.close()
                server.shutdown()
            }
        }

    @Test
    fun `ticket validation pins workspace protocol route generation and expiry`() {
        val ticket =
            obj(
                "workspaceId" to "workspace".json(),
                "wsUrl" to "wss://cloud.example/v1/cloud/workspaces/workspace/gateway".json(),
                "protocol" to CloudGateway.RUST.json(),
                "rpcProtocol" to CloudGateway.RPC.json(),
                "credential" to "private-ticket".json(),
                "generation" to 1.json(),
                "gatewayEpoch" to 1.json(),
                "expiresAt" to (System.currentTimeMillis() + 60_000).json(),
            )
        CloudGateway.validate(ticket, "workspace", CloudGateway.RUST)
        assertEquals(
            CloudGateway.RUST,
            CloudGateway.protocol(obj("rpcProtocol" to CloudGateway.RPC.json())),
        )
        for ((key, value) in
            listOf(
                "workspaceId" to "other".json(),
                "rpcProtocol" to CloudGateway.EFFECT.json(),
                "generation" to 0.json(),
                "gatewayEpoch" to 0.json(),
                "expiresAt" to 0.json(),
                "wsUrl" to "wss://cloud.example/other?token=leak".json(),
            )) {
            assertFails {
                CloudGateway.validate(
                    kotlinx.serialization.json.JsonObject(ticket + (key to value)),
                    "workspace",
                    CloudGateway.RUST,
                )
            }
        }
        assertFails { CloudGateway.protocol(obj("rpcProtocol" to "unknown".json())) }
    }
}
