package sh.zuse.core

import kotlin.test.*
import kotlinx.coroutines.runBlocking
import okhttp3.*
import okhttp3.mockwebserver.*
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate

class CloudClientTests {
    @Test
    fun `local account HTTP requires explicit opt in and stays on its origin`() = runBlocking {
        val server = MockWebServer()
        server.start()
        try {
            val origin = "http://127.0.0.1:${server.port}"
            var credentials = 0
            val token: suspend () -> String = { credentials++; "test-account" }
            assertFailsWith<IllegalArgumentException> { CloudClient(origin, token).request("/v1/environments") }
            assertEquals(0, credentials)
            val local = CloudClient(origin, token, allowLoopbackHttp = true)
            server.enqueue(MockResponse().setBody("{\"environments\":[]}"))
            assertTrue(local.environments().isEmpty())
            assertEquals("Bearer test-account", server.takeRequest().getHeader("Authorization"))
            for (target in listOf("http://example.test", "http://localhost:${server.port}", "http://192.168.1.2")) {
                assertFailsWith<IllegalArgumentException> { CloudClient(target, token, allowLoopbackHttp = true).request("/") }
            }
            assertFailsWith<IllegalArgumentException> { local.request("http://example.test/steal") }
            assertEquals(1, credentials)
        } finally { server.shutdown() }
    }
    @Test
    fun `Rust cloud workspace wakes and negotiates canonical gateway without legacy handshake`() =
        runBlocking {
            val certificate =
                HeldCertificate.Builder().addSubjectAlternativeName("localhost").build()
            val tls = HandshakeCertificates.Builder().heldCertificate(certificate).build()
            val trust =
                HandshakeCertificates.Builder()
                    .addTrustedCertificate(certificate.certificate)
                    .build()
            val server = MockWebServer()
            server.useHttps(tls.sslSocketFactory(), false)
            server.start()
            val ticket =
                obj(
                    "workspaceId" to "workspace".json(),
                    "wsUrl" to
                        server
                            .url("/v1/cloud/workspaces/workspace/gateway")
                            .toString()
                            .replaceFirst("https://", "wss://")
                            .json(),
                    "protocol" to CloudGateway.RUST.json(),
                    "rpcProtocol" to CloudGateway.RPC.json(),
                    "credential" to "private-ticket".json(),
                    "generation" to 1.json(),
                    "gatewayEpoch" to 1.json(),
                    "expiresAt" to (System.currentTimeMillis() + 60_000).json(),
                )
            server.enqueue(
                MockResponse()
                    .setBody(
                        obj(
                                "rpcProtocol" to CloudGateway.RPC.json(),
                                "state" to "paused".json(),
                                "desiredState" to "paused".json(),
                            )
                            .toString()
                    )
            )
            server.enqueue(MockResponse().setBody("""{"workspaceId":"workspace"}"""))
            server.enqueue(MockResponse().setBody(ticket.toString()))
            server.enqueue(
                MockResponse()
                    .setHeader("Sec-WebSocket-Protocol", CloudGateway.RUST)
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onOpen(webSocket: WebSocket, response: Response) {
                                webSocket.send(
                                    obj(
                                            "_zuseGateway" to "availability".json(),
                                            "state" to "available".json(),
                                            "reset" to false.json(),
                                        )
                                        .toString()
                                )
                            }

                            override fun onMessage(webSocket: WebSocket, text: String) {
                                val frame = WireJson.parseToJsonElement(text)
                                assertEquals("EngineInfo", frame.text("method"))
                                webSocket.send(
                                    obj(
                                            "id" to frame.field("id"),
                                            "ok" to obj("deviceId" to "guest".json()),
                                        )
                                        .toString()
                                )
                            }
                        }
                    )
            )
            val http =
                RpcClient.defaultHttp()
                    .newBuilder()
                    .sslSocketFactory(trust.sslSocketFactory(), trust.trustManager)
                    .build()
            val cloud = CloudClient(server.url("/").toString(), { "account" }, http)
            try {
                val rpc = cloud.connect("workspace")
                try {
                    assertTrue(rpc.canonical)
                    assertEquals("/v1/cloud/workspaces/workspace", server.takeRequest().path)
                    val wake = server.takeRequest()
                    assertEquals("/v1/cloud/workspaces/workspace/resume", wake.path)
                    assertEquals(
                        "workspace",
                        WireJson.parseToJsonElement(wake.body.readUtf8()).text("workspaceId"),
                    )
                    val request = server.takeRequest()
                    val body = WireJson.parseToJsonElement(request.body.readUtf8())
                    assertEquals(CloudGateway.RUST, body.text("protocol"))
                    assertEquals(CloudGateway.RPC, body.text("rpcProtocol"))
                    val upgrade = server.takeRequest()
                    assertNull(upgrade.requestUrl!!.query)
                    assertEquals(
                        "${CloudGateway.RUST}, private-ticket",
                        upgrade.getHeader("Sec-WebSocket-Protocol"),
                    )
                } finally {
                    rpc.close()
                }
            } finally {
                server.shutdown()
            }
        }

    @Test
    fun `managed computer flow registers bound device and uses canonical account socket`() =
        runBlocking {
            val certificate =
                HeldCertificate.Builder().addSubjectAlternativeName("localhost").build()
            val serverTls = HandshakeCertificates.Builder().heldCertificate(certificate).build()
            val trusted =
                HandshakeCertificates.Builder()
                    .addTrustedCertificate(certificate.certificate)
                    .build()
            val server = MockWebServer()
            server.useHttps(serverTls.sslSocketFactory(), false)
            server.start()
            val expiry = System.currentTimeMillis() + 60_000
            server.enqueue(MockResponse().setBody("""{"accessToken":"device-access"}"""))
            server.enqueue(MockResponse().setBody("{}"))
            val origin = server.url("/").toString().replaceFirst("https://", "wss://")
            server.enqueue(
                MockResponse()
                    .setBody(
                        obj(
                                "endpoint" to obj("wsBaseUrl" to origin.json()),
                                "connectToken" to "host-grant".json(),
                                "expiresAt" to expiry.json(),
                            )
                            .toString()
                    )
            )
            server.enqueue(
                MockResponse()
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onMessage(webSocket: WebSocket, text: String) {
                                val frame = WireJson.parseToJsonElement(text)
                                if (frame.text("protocol") != null) {
                                    assertEquals("host-grant", frame.text("connectToken"))
                                    webSocket.send(
                                        obj(
                                                "protocol" to AccountSocket.PROTOCOL.json(),
                                                "authenticated" to true.json(),
                                                "deviceId" to "host".json(),
                                                "expiresAt" to expiry.json(),
                                            )
                                            .toString()
                                    )
                                } else
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
            val proofs = java.util.concurrent.CopyOnWriteArrayList<Pair<String, String?>>()
            val http =
                RpcClient.defaultHttp()
                    .newBuilder()
                    .sslSocketFactory(trusted.sslSocketFactory(), trusted.trustManager)
                    .build()
            val cloud = CloudClient(server.url("/").toString(), { "account" }, http)
            val header =
                java.util.Base64.getUrlEncoder()
                    .withoutPadding()
                    .encodeToString(
                        """{"jwk":{"kty":"EC","crv":"P-256","x":"x","y":"y"}}""".toByteArray()
                    )
            try {
                val rpc =
                    cloud.connectEnvironment(
                        "host",
                        { _, address, access ->
                            proofs.add(address to access)
                            "$header.payload.signature"
                        },
                        canonicalAccount = true,
                    )
                try {
                    assertTrue(rpc.canonical)
                    val token = server.takeRequest()
                    assertEquals("/v1/client/dpop-token", token.path)
                    assertEquals("Bearer account", token.getHeader("Authorization"))
                    val register = server.takeRequest()
                    assertEquals("/v1/mobile/devices", register.path)
                    assertEquals("DPoP device-access", register.getHeader("Authorization"))
                    val connect = server.takeRequest()
                    assertEquals("/v1/environments/host/connect", connect.path)
                    val body = WireJson.parseToJsonElement(connect.body.readUtf8())
                    assertTrue(body.flag("requireManaged"))
                    assertEquals(1L, body.long("wireProtocolVersion"))
                    assertEquals("/api/account-rpc", server.takeRequest().path)
                    assertNull(proofs.first().second)
                    assertTrue(proofs.drop(1).all { it.second == "device-access" })
                } finally {
                    rpc.close()
                }
            } finally {
                server.shutdown()
            }
        }

    @Test
    fun `cloud requests keep bearer tokens on the configured secure origin and reject redirects`() =
        runBlocking {
            val certificate =
                HeldCertificate.Builder().addSubjectAlternativeName("localhost").build()
            val serverTls = HandshakeCertificates.Builder().heldCertificate(certificate).build()
            val trusted =
                HandshakeCertificates.Builder()
                    .addTrustedCertificate(certificate.certificate)
                    .build()
            val server = MockWebServer()
            server.useHttps(serverTls.sslSocketFactory(), false)
            server.enqueue(MockResponse().setBody("{\"workspaces\":[]}"))
            server.enqueue(
                MockResponse()
                    .setResponseCode(302)
                    .setHeader("Location", "https://other.example/leak")
            )
            server.start()
            try {
                val http =
                    RpcClient.defaultHttp()
                        .newBuilder()
                        .sslSocketFactory(trusted.sslSocketFactory(), trusted.trustManager)
                        .build()
                val cloud = CloudClient(server.url("/").toString(), { "account-secret" }, http)
                assertEquals(emptyList(), cloud.chats())
                val request = server.takeRequest()
                assertEquals("/v1/cloud/workspaces", request.path)
                assertEquals("Bearer account-secret", request.getHeader("Authorization"))
                assertFails { cloud.request("/redirect") }
                assertFails { cloud.request("https://other.example/leak") }
                assertEquals(2, server.requestCount)
            } finally {
                server.shutdown()
            }
        }
}
