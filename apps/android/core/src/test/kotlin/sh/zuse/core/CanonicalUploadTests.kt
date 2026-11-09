package sh.zuse.core

import kotlin.test.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.mockwebserver.*

class CanonicalUploadTests {
    @Test
    fun `host upload reassembles exact bytes before commit and returns host reference`() =
        runBlocking {
            val received = java.io.ByteArrayOutputStream()
            var sequence = 0L
            val server = MockWebServer()
            server.enqueue(
                MockResponse()
                    .withWebSocketUpgrade(
                        object : WebSocketListener() {
                            override fun onMessage(socket: WebSocket, text: String) {
                                val frame = WireJson.parseToJsonElement(text)
                                val params = frame.field("params")
                                val result =
                                    when (frame.text("method")) {
                                        "EngineInfo" -> obj()
                                        "UploadChunk" -> {
                                            assertEquals(sequence++, params.long("seq"))
                                            received.write(
                                                java.util.Base64.getDecoder()
                                                    .decode(params.text("data"))
                                            )
                                            obj("ok" to true.json())
                                        }
                                        "UploadCommit" -> {
                                            assertEquals("sample.bin", params.text("fileName"))
                                            obj("path" to "/host/uploads/sample.bin".json())
                                        }
                                        "ReadAttachmentChunk" -> {
                                            if (params.text("path") == "/invalid-offset") {
                                                socket.send(
                                                    obj(
                                                            "id" to frame.field("id"),
                                                            "ok" to
                                                                obj(
                                                                    "data" to "AA==".json(),
                                                                    "nextOffset" to 0.json(),
                                                                    "done" to false.json(),
                                                                ),
                                                        )
                                                        .toString()
                                                )
                                                return
                                            }
                                            val start = params.long("offset")!!.toInt()
                                            val all = received.toByteArray()
                                            val end = (start + 45 * 1024).coerceAtMost(all.size)
                                            obj(
                                                "data" to
                                                    java.util.Base64.getEncoder()
                                                        .encodeToString(all.copyOfRange(start, end))
                                                        .json(),
                                                "nextOffset" to end.json(),
                                                "done" to (end == all.size).json(),
                                            )
                                        }
                                        else -> error("Unexpected method")
                                    }
                                socket.send(
                                    obj("id" to frame.field("id"), "ok" to result).toString()
                                )
                            }
                        }
                    )
            )
            server.start()
            val client = RpcClient(server.url("/api/rpc").toString().replace("http:", "ws:"))
            val file = java.io.File.createTempFile("zuse-upload", ".bin")
            try {
                val bytes = ByteArray(130_001) { (it % 251).toByte() }
                file.writeBytes(bytes)
                client.connect()
                val ref =
                    CanonicalWorkspace(client)
                        .upload(file, "sample.bin", "application/octet-stream")
                assertContentEquals(bytes, received.toByteArray())
                assertTrue(sequence > 1)
                assertEquals("/host/uploads/sample.bin", ref.text("path"))
                assertEquals("sample.bin", ref.text("originalName"))
                val downloaded = java.io.ByteArrayOutputStream()
                CanonicalWorkspace(client).download(ref.text("path")!!, downloaded)
                assertContentEquals(bytes, downloaded.toByteArray())
                val rejected = java.io.ByteArrayOutputStream()
                assertFailsWith<IllegalArgumentException> {
                    CanonicalWorkspace(client).download("/invalid-offset", rejected)
                }
                assertEquals(0, rejected.size())
            } finally {
                file.delete()
                client.close()
                server.shutdown()
            }
        }
}
