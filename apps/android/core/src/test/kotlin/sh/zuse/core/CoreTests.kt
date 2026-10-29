package sh.zuse.core

import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.mockwebserver.*

class CoreTests {
    private fun frame(kind: String, vararg fields: Pair<String, JsonElement?>) =
        obj("kind" to kind.json(), "sessionId" to "s".json(), *fields)

    private fun message(id: String, text: String = id) =
        Message(id, "s", "assistant", obj("_tag" to "text".json(), "text" to text.json()))

    private fun snapshot(messages: List<Message> = emptyList()) =
        frame(
            "snapshot",
            "throughVersion" to 4.json(),
            "cursor" to obj("epoch" to "a".json()),
            "projection" to obj("messages" to WireJson.encodeToJsonElement(messages)),
        )

    @Test
    fun `large transcript preserves identities and merges updates and pages`() {
        val transcript = Transcript()
        transcript.apply(snapshot((100..10099).map { message("m$it") }), "s")
        val updated = message("m10099", "changed")
        transcript.apply(
            frame(
                "event",
                "streamVersion" to 5.json(),
                "event" to
                    obj(
                        "_tag" to "MessagePersisted".json(),
                        "message" to WireJson.encodeToJsonElement(updated),
                    ),
            ),
            "s",
        )
        transcript.prependPage(
            obj(
                "messages" to
                    WireJson.encodeToJsonElement((0..100).reversed().map { message("m$it") }),
                "olderMessageSequence" to JsonNull,
            ),
            "s",
        )
        assertEquals(10100, transcript.messages.size)
        assertEquals("m0", transcript.messages.first().id)
        assertEquals("changed", transcript.messages.last().text)
        assertNull(transcript.olderSequence)
    }

    @Test
    fun `gaps restart epochs and cross session frames fail closed`() {
        val transcript = Transcript()
        transcript.apply(snapshot(), "s")
        assertFails { transcript.apply(frame("event", "streamVersion" to 6.json()), "s") }
        assertFails {
            transcript.apply(
                frame(
                    "synchronized",
                    "throughVersion" to 4.json(),
                    "cursor" to obj("epoch" to "b".json()),
                ),
                "s",
            )
        }
        assertFails { transcript.apply(snapshot(), "other") }
        transcript.apply(frame("event", "streamVersion" to 4.json()), "s")
        assertEquals(4, transcript.version)
    }

    @Test
    fun `cleartext permits local and tailscale but rejects public and credentials`() {
        listOf(
                "ws://192.168.1.5:8080/rpc",
                "ws://100.64.0.2/rpc",
                "ws://mac.tail123.ts.net/rpc",
                "ws://[fd7a:115c:a1e0::1]/rpc",
            )
            .forEach { Endpoint.validate(it) }
        listOf(
                "ws://example.com/rpc",
                "ws://100.128.1.1/rpc",
                "wss://user:password@example.com/rpc",
                "file:///tmp/token",
                "wss://example.com/rpc#secret",
            )
            .forEach { assertFails(it) { Endpoint.validate(it) } }
        val pairing = Endpoint.pairing("http://192.168.1.4:8080/#pair=one-time-code")
        assertEquals("ws://192.168.1.4:8080/rpc", pairing.websocket)
        assertEquals("one-time-code", pairing.code)
        assertFails { Endpoint.pairing("https://example.com/#pair=") }
        assertFails { Endpoint.pairing("https://user:password@example.com/#pair=code") }
        assertEquals(
            "wss://example.com/rpc",
            Endpoint.pairing("zuse:///connect/pair?pairingUrl=wss%3A%2F%2Fexample.com#token=code")
                .websocket,
        )
    }

    @Test
    fun `diff line numbers cover additions deletions zero ranges and no newline markers`() {
        val lines =
            DiffLine.parse(
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -2,2 +2,2 @@\n same\n-old\n+new\n\\ No newline at end of file"
            )
        assertEquals(3, lines[5].oldLine)
        assertEquals(3, lines[6].newLine)
        assertEquals(DiffLine.Kind.HEADER, lines[7].kind)
        assertEquals(1, DiffLine.parse("@@ -0,0 +1 @@\n+new").last().newLine)
    }

    @Test
    fun `websocket handshake chunk acknowledgement cancellation and disconnect cleanup`() =
        runBlocking {
            val server = MockWebServer()
            val received = LinkedBlockingQueue<JsonElement>()
            var peer: WebSocket? = null
            server.enqueue(
                MockResponse()
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onOpen(webSocket: WebSocket, response: Response) {
                                peer = webSocket
                            }

                            override fun onMessage(webSocket: WebSocket, text: String) {
                                val request = WireJson.parseToJsonElement(text)
                                received.add(request)
                                if (request.text("tag") == "connect.handshake")
                                    webSocket.send(
                                        obj(
                                                "_tag" to "Exit".json(),
                                                "requestId" to request.field("id"),
                                                "exit" to
                                                    obj(
                                                        "_tag" to "Success".json(),
                                                        "value" to
                                                            obj("protocolVersion" to 7.json()),
                                                    ),
                                            )
                                            .toString()
                                    )
                                if (request.text("tag") == "reject")
                                    webSocket.send(
                                        obj(
                                                "_tag" to "Exit".json(),
                                                "requestId" to request.field("id"),
                                                "exit" to
                                                    obj(
                                                        "_tag" to "Failure".json(),
                                                        "cause" to
                                                            JsonArray(
                                                                listOf(
                                                                    obj(
                                                                        "_tag" to "Fail".json(),
                                                                        "error" to
                                                                            obj(
                                                                                "_tag" to
                                                                                    "NotAvailable"
                                                                                        .json(),
                                                                                "message" to
                                                                                    "Choose another provider."
                                                                                        .json(),
                                                                            ),
                                                                    )
                                                                )
                                                            ),
                                                    ),
                                            )
                                            .toString()
                                    )
                                if (request.text("tag") == "session.events")
                                    webSocket.send(
                                        obj(
                                                "_tag" to "Chunk".json(),
                                                "requestId" to request.field("id"),
                                                "values" to JsonArray(listOf("one".json())),
                                            )
                                            .toString()
                                    )
                            }
                        }
                    )
            )
            server.start()
            val client = RpcClient(server.url("/rpc").toString().replace("http://", "ws://"))
            try {
                client.connect()
                assertEquals(listOf("one".json()), client.stream("session.events").take(1).toList())
                val tags =
                    withContext(Dispatchers.IO) {
                        List(4) { received.poll(5, TimeUnit.SECONDS)?.text("_tag") }
                    }
                assertTrue("Ack" in tags)
                assertTrue("Interrupt" in tags)
                val rejected = assertFailsWith<RpcRequestFailure> { client.call("reject") }
                assertEquals("NotAvailable", rejected.tag)
                withContext(Dispatchers.IO) { received.poll(5, TimeUnit.SECONDS) }
                val waiting = async { runCatching { client.call("pending") } }
                withContext(Dispatchers.IO) { received.poll(5, TimeUnit.SECONDS) }
                peer!!.close(1001, "restart")
                assertTrue(withTimeout(5000) { waiting.await() }.isFailure)
            } finally {
                client.close()
                server.shutdown()
            }
        }
}

class CloudSecretTests {
    @Test
    fun `cloud credentials are RSA OAEP SHA256 sealed with SHA256 MGF1`() {
        val pair =
            java.security.KeyPairGenerator.getInstance("RSA")
                .apply { initialize(2048) }
                .generateKeyPair()
        val public = pair.public as java.security.interfaces.RSAPublicKey
        val encode = java.util.Base64.getUrlEncoder().withoutPadding()
        val jwk =
            obj(
                "kty" to "RSA".json(),
                "n" to
                    encode
                        .encodeToString(
                            public.modulus
                                .toByteArray()
                                .dropWhile { it == 0.toByte() }
                                .toByteArray()
                        )
                        .json(),
                "e" to encode.encodeToString(public.publicExponent.toByteArray()).json(),
            )
        val sealed = CloudSecret.seal("private-api-key", jwk.toString())
        assertFalse(sealed.contains("private-api-key"))
        val cipher = javax.crypto.Cipher.getInstance("RSA/ECB/OAEPPadding")
        cipher.init(
            javax.crypto.Cipher.DECRYPT_MODE,
            pair.private,
            javax.crypto.spec.OAEPParameterSpec(
                "SHA-256",
                "MGF1",
                java.security.spec.MGF1ParameterSpec.SHA256,
                javax.crypto.spec.PSource.PSpecified.DEFAULT,
            ),
        )
        assertEquals(
            "private-api-key",
            cipher.doFinal(java.util.Base64.getUrlDecoder().decode(sealed)).toString(Charsets.UTF_8),
        )
        assertFails { CloudSecret.seal("secret", obj("kty" to "EC".json()).toString()) }
    }
}

class MarkdownBlockTests {
    @Test
    fun `diagram extraction preserves incomplete nested and ordinary code fences`() {
        val blocks = MarkdownBlock.parse("Before\n```mermaid\ngraph TD; A-->B\n```\nAfter")
        assertEquals(listOf(false, true, false), blocks.map { it.diagram })
        assertEquals("graph TD; A-->B", blocks[1].content)
        val incomplete = "Before\n```mermaid\ngraph TD; A-->B"
        assertEquals(listOf(MarkdownBlock(false, incomplete)), MarkdownBlock.parse(incomplete))
        val nested = "````text\n```mermaid\ngraph TD; A-->B\n```\n````"
        val extracted = MarkdownBlock.parse(nested).single()
        assertFalse(extracted.diagram)
        assertEquals("text", extracted.language)
        assertEquals("```mermaid\ngraph TD; A-->B\n```", extracted.content)
        val code = MarkdownBlock.parse("~~~kotlin\nval n = 1\n~~~").single()
        assertEquals("kotlin", code.language)
        assertEquals("val n = 1", code.content)
    }
}

class UiTreeTests {
    @Test
    fun `forms collect actual defaults and preserve boolean and numeric answers`() {
        val root =
            WireJson.parseToJsonElement(
                """{"typeName":"Form","props":{"children":[{"typeName":"Input","props":{"name":"title","value":"Default"}},{"typeName":"Checkbox","props":{"name":"enabled","checked":true}},{"typeName":"Select","props":{"name":"size","options":["small","large"]}},{"typeName":"Slider","props":{"name":"count","min":1,"max":10}}]}}"""
            )
        assertEquals(
            obj(
                "title" to "Changed".json(),
                "enabled" to true.json(),
                "size" to "small".json(),
                "count" to 1.json(),
            ),
            collectUiAnswers(root, mapOf("title" to "Changed".json())),
        )
        var nested: JsonElement =
            obj("typeName" to "Text".json(), "props" to obj("text" to "leaf".json()))
        repeat(25) {
            nested =
                obj(
                    "typeName" to "Card".json(),
                    "props" to obj("children" to JsonArray(listOf(nested))),
                )
        }
        assertFails { validateUiTree(nested) }
    }
}

class RpcCancellationTests {
    @Test
    fun `canceling a consumer during a chunk preserves the connection for other requests`() =
        runBlocking {
            val server = MockWebServer()
            server.enqueue(
                MockResponse()
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onMessage(socket: WebSocket, text: String) {
                                val request = WireJson.parseToJsonElement(text)
                                if (request.text("_tag") != "Request") return
                                val id = request.field("id")
                                if (request.text("tag") == "cancel-race") {
                                    socket.send(
                                        obj(
                                                "_tag" to "Chunk".json(),
                                                "requestId" to id,
                                                "values" to
                                                    JsonArray((0 until 128).map { it.json() }),
                                            )
                                            .toString()
                                    )
                                } else {
                                    val value =
                                        if (request.text("tag") == "connect.handshake")
                                            obj("protocolVersion" to 7.json())
                                        else "ok".json()
                                    socket.send(
                                        obj(
                                                "_tag" to "Exit".json(),
                                                "requestId" to id,
                                                "exit" to
                                                    obj(
                                                        "_tag" to "Success".json(),
                                                        "value" to value,
                                                    ),
                                            )
                                            .toString()
                                    )
                                }
                            }
                        }
                    )
            )
            server.start()
            val client = RpcClient(server.url("/rpc").toString().replace("http://", "ws://"))
            try {
                client.connect()
                withTimeout(10_000) {
                    repeat(64) {
                        assertEquals(
                            listOf(0.json()),
                            client.stream("cancel-race").take(1).toList(),
                        )
                    }
                }
                assertEquals("ok".json(), client.call("health"))
            } finally {
                client.close()
                server.shutdown()
            }
        }
}

class ArchiveMetadataTests {
    @Test
    fun `legacy history and pending work remain recoverable while metadata becomes small`() {
        val messages =
            (0 until 10_000).map {
                Message(
                    "m$it",
                    "s",
                    "assistant",
                    obj("_tag" to "assistant".json(), "text" to "Cached history $it".json()),
                )
            }
        val pending = PendingMessage("command", "message", "s", "Keep this draft")
        val legacy =
            obj(
                "messages" to WireJson.encodeToJsonElement(messages),
                "draft" to "Unsent text".json(),
                "pending" to WireJson.encodeToJsonElement(listOf(pending)),
            )
        val decoded = decodeSessionMetadata(legacy.toString())
        assertEquals(10_000, decoded.legacyMessages!!.size)
        assertEquals("Unsent text", decoded.metadata.draft)
        assertEquals(listOf(pending), decoded.metadata.pending)
        val updated =
            decoded.metadata.copy(
                draft = "Changed",
                uiDrafts = mapOf("visual" to mapOf("name" to "Native".json())),
            )
        val encoded = WireJson.encodeToJsonElement(updated)
        assertEquals(JsonNull, encoded.field("messages"))
        assertTrue(encoded.toString().length < 1000)
        assertEquals("Native", encoded.field("uiDrafts").field("visual").text("name"))
        assertFails { decodeSessionMetadata("{\"messages\":\"damaged\"}") }
    }
}

class InputRevisionTests {
    @Test
    fun `late saves cannot resurrect submitted drafts or erase independent form edits`() {
        val initial =
            SessionMetadata(
                draftRevision = 1,
                formRevision = 1,
                draft = "Submit me",
                uiDrafts = mapOf("f" to mapOf("name" to "First".json())),
            )
        val sent = initial.withInputs(SessionInputs(2, "", 1, initial.uiDrafts))
        val editedForm =
            sent.withInputs(
                SessionInputs(2, "Submit me", 2, mapOf("f" to mapOf("name" to "Changed".json())))
            )
        assertEquals("", editedForm.draft)
        assertEquals("Changed", (editedForm.uiDrafts["f"]?.get("name") as JsonPrimitive).content)
        val newer = editedForm.withInputs(SessionInputs(3, "Next prompt", 2, editedForm.uiDrafts))
        assertEquals("Next prompt", newer.withInputs(SessionInputs(2, "Old", 1, emptyMap())).draft)
        val responded = newer.copy(responses = mapOf("f" to "command"))
        assertFalse(
            responded
                .withInputs(SessionInputs(3, "", 3, editedForm.uiDrafts))
                .uiDrafts
                .containsKey("f")
        )
        val legacy = decodeSessionMetadata("{\"inputRevision\":17,\"draft\":\"Keep me\"}").metadata
        assertEquals(17L, legacy.draftRevision)
        assertEquals(17L, legacy.formRevision)
    }
}
