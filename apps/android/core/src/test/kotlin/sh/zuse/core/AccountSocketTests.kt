package sh.zuse.core

import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlinx.coroutines.*
import okhttp3.*
import okhttp3.mockwebserver.*
import org.junit.Test

class AccountSocketTests {
    @Test
    fun `idle account socket cannot outlive grant expiry`() = runBlocking {
        val server = MockWebServer()
        val expiry = System.currentTimeMillis() + 1800
        server.enqueue(
            MockResponse()
                .withWebSocketUpgrade(
                    object : WebSocketListener() {
                        override fun onMessage(webSocket: WebSocket, text: String) {
                            val frame = WireJson.parseToJsonElement(text)
                            if (frame.text("protocol") != null)
                                webSocket.send(
                                    obj(
                                            "protocol" to AccountSocket.PROTOCOL.json(),
                                            "authenticated" to true.json(),
                                            "deviceId" to "host".json(),
                                            "expiresAt" to expiry.json(),
                                        )
                                        .toString()
                                )
                            else
                                webSocket.send(
                                    obj(
                                            "id" to frame.field("id"),
                                            "ok" to obj("deviceId" to "host".json()),
                                        )
                                        .toString()
                                )
                        }
                    }
                )
        )
        server.start()
        val client =
            RpcClient(
                server.url("/").toString(),
                accountEnvironmentId = "host",
                accountGrant = { AccountGrant("host", server.url("/").toString(), "grant", expiry) },
            )
        try {
            client.connect()
            withTimeout(5000) { client.awaitDisconnect() }
            assertFails { client.call("EngineInfo") }
            Unit
        } finally {
            client.close()
            server.shutdown()
        }
    }

    @Test
    fun `account authenticates before RPC and renews without exposing grants in URL`() =
        runBlocking {
            val server = MockWebServer()
            val grants = AtomicInteger()
            val renewal = CompletableDeferred<Unit>()
            val originalExpiry = System.currentTimeMillis() + 2400
            server.enqueue(
                MockResponse()
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onMessage(webSocket: WebSocket, text: String) {
                                val value = WireJson.parseToJsonElement(text)
                                if (value.text("protocol") != null) {
                                    assertEquals(AccountSocket.PROTOCOL, value.text("protocol"))
                                    val renewed = value.text("connectToken") == "secret-renewed"
                                    val expiry =
                                        if (renewed) originalExpiry + 60_000 else originalExpiry
                                    webSocket.send(
                                        obj(
                                                "protocol" to AccountSocket.PROTOCOL.json(),
                                                "authenticated" to true.json(),
                                                "deviceId" to "host".json(),
                                                "expiresAt" to expiry.json(),
                                                "renewed" to renewed.json(),
                                            )
                                            .toString()
                                    )
                                    if (renewed) renewal.complete(Unit)
                                } else {
                                    assertEquals("EngineInfo", value.text("method"))
                                    webSocket.send(
                                        obj(
                                                "id" to value.field("id"),
                                                "ok" to obj("deviceId" to "host".json()),
                                            )
                                            .toString()
                                    )
                                }
                            }
                        }
                    )
            )
            server.start()
            val client =
                RpcClient(
                    server.url("/").toString(),
                    accountEnvironmentId = "host",
                    accountGrant = {
                        val initial = grants.getAndIncrement() == 0
                        AccountGrant(
                            "host",
                            server.url("/").toString(),
                            if (initial) "secret-initial" else "secret-renewed",
                            if (initial) originalExpiry else originalExpiry + 60_000,
                        )
                    },
                )
            try {
                client.connect()
                assertEquals("host", client.call("EngineInfo").text("deviceId"))
                withTimeout(8000) { renewal.await() }
                assertEquals("host", client.call("EngineInfo").text("deviceId"))
                val upgrade = server.takeRequest()
                assertEquals("/api/account-rpc", upgrade.path)
                assertEquals(AccountSocket.PROTOCOL, upgrade.getHeader("Sec-WebSocket-Protocol"))
                assertNull(upgrade.getHeader("Authorization"))
            } finally {
                client.close()
                server.shutdown()
            }
        }

    @Test
    fun `account refuses wrong identity and RPC cannot bypass authentication`() = runBlocking {
        val server = MockWebServer()
        val rpcCount = AtomicInteger()
        server.enqueue(
            MockResponse()
                .withWebSocketUpgrade(
                    object : WebSocketListener() {
                        override fun onMessage(webSocket: WebSocket, text: String) {
                            val frame = WireJson.parseToJsonElement(text)
                            if (frame.text("method") != null) rpcCount.incrementAndGet()
                            webSocket.send(
                                obj(
                                        "protocol" to AccountSocket.PROTOCOL.json(),
                                        "authenticated" to true.json(),
                                        "deviceId" to "imposter".json(),
                                        "expiresAt" to (System.currentTimeMillis() + 30_000).json(),
                                    )
                                    .toString()
                            )
                        }
                    }
                )
        )
        server.start()
        val client =
            RpcClient(
                server.url("/").toString(),
                accountEnvironmentId = "host",
                accountGrant = {
                    AccountGrant(
                        "host",
                        server.url("/").toString(),
                        "secret",
                        System.currentTimeMillis() + 60_000,
                    )
                },
            )
        try {
            assertFails { client.connect() }
            assertEquals(0, rpcCount.get())
            assertFails { client.call("EngineInfo") }
            Unit
        } finally {
            client.close()
            server.shutdown()
        }
    }

    @Test
    fun `grant validation rejects remote plaintext origins and secrets stay out of diagnostics`() {
        val grant =
            AccountGrant(
                "host",
                "http://host.example",
                "private-secret",
                System.currentTimeMillis() + 60_000,
            )
        assertFails { grant.socketUrl() }
        assertFalse(grant.toString().contains("private-secret"))
        assertFalse(
            RemoteEnvironment("host", providerKind = "desktop", wireProtocolVersion = 1)
                .canonicalAccount
        )
        assertTrue(
            RemoteEnvironment(
                    "host",
                    providerKind = "desktop",
                    wireProtocolVersion = 1,
                    capabilities =
                        obj(
                            "features" to
                                kotlinx.serialization.json.JsonArray(
                                    listOf(AccountSocket.PROTOCOL.json())
                                )
                        ),
                )
                .canonicalAccount
        )
    }
}
