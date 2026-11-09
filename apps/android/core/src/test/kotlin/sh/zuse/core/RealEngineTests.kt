package sh.zuse.core

import kotlin.test.*
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.*
import org.junit.Assume.assumeTrue

/** Explicit local acceptance gate: never contacts a production service by default. */
class RealEngineTests {
    @Test
    fun `native client reads real engine creates chat and round trips host attachment`() =
        runBlocking {
            val address = System.getenv("ZUSE_NATIVE_RPC_ADDRESS")
            assumeTrue(
                "Set ZUSE_NATIVE_RPC_ADDRESS for real-engine acceptance",
                !address.isNullOrBlank(),
            )
            require(java.net.URI(address).host in listOf("localhost", "127.0.0.1", "::1"))
            val client = RpcClient(address!!, token = System.getenv("ZUSE_NATIVE_RPC_TOKEN"))
            val file = java.io.File.createTempFile("native-acceptance", ".txt")
            try {
                client.connect()
                assertNotNull(client.call("EngineInfo").text("deviceId"))
                val workspace = CanonicalWorkspace(client)
                assertTrue(workspace.projects().any { it.id == CanonicalWorkspace.PROJECTLESS })
                val id = java.util.UUID.randomUUID().toString()
                val chat =
                    workspace.create(
                        obj(
                            "operationId" to id.json(),
                            "projectId" to CanonicalWorkspace.PROJECTLESS.json(),
                            "providerId" to "mock".json(),
                            "model" to "mock-1".json(),
                        ),
                        null,
                        false,
                    )
                assertEquals(id, chat.id)
                assertTrue(workspace.sessions(CanonicalWorkspace.PROJECTLESS).any { it.id == id })
                workspace.rename(id, "Native renamed chat")
                assertEquals("Native renamed chat", workspace.session(id).title)
                workspace.setArchived(id, true)
                assertFalse(workspace.sessions(CanonicalWorkspace.PROJECTLESS).any { it.id == id })
                assertTrue(workspace.archivedSessions().any { it.id == id })
                workspace.setArchived(id, false)
                assertTrue(workspace.sessions(CanonicalWorkspace.PROJECTLESS).any { it.id == id })
                workspace.send(
                    PendingMessage(
                        java.util.UUID.randomUUID().toString(),
                        java.util.UUID.randomUUID().toString(),
                        id,
                        "Exercise native question response",
                    )
                )
                val transcript = CanonicalTranscript()
                withTimeout(15_000) {
                    client.stream("WatchDocMessages", obj("chatId" to id.json())).first { update ->
                        transcript.apply(update)
                        transcript.interactions().isNotEmpty()
                    }
                }
                val interaction = transcript.interactions().first()
                val questions = interaction.array("questions")
                workspace.answer(
                    id,
                    requireNotNull(interaction.text("id")),
                    questions,
                    JsonArray(
                        questions.indices.map { index ->
                            obj(
                                "questionIndex" to index.json(),
                                "selected" to JsonArray(listOf(0.json())),
                            )
                        }
                    ),
                )
                val completed = CanonicalTranscript()
                withTimeout(15_000) {
                    client.stream("WatchDocMessages", obj("chatId" to id.json())).first { update ->
                        completed.apply(update)
                        completed.messages(id).any {
                            it.content.text("text")?.contains("Locked in:") == true
                        }
                    }
                }
                file.writeText("native host attachment · UTF-8 🌍")
                val ref = workspace.upload(file, "native-acceptance.txt", "text/plain")
                val output = java.io.ByteArrayOutputStream()
                workspace.download(ref.text("path")!!, output)
                assertContentEquals(file.readBytes(), output.toByteArray())
                val grant = client.call("CreatePairing", obj("label" to "Native acceptance".json()))
                val uri = java.net.URI(address)
                val origin = "http://${uri.rawAuthority}"
                val link = "$origin/${requireNotNull(grant.text("fragment"))}"
                val paired = RpcClient(address, pairingLink = link)
                try {
                    paired.connect()
                    assertEquals(
                        client.call("EngineInfo").text("deviceId"),
                        paired.call("EngineInfo").text("deviceId"),
                    )
                    val ref = CanonicalWorkspace(paired).upload(file, "paired.txt", "text/plain")
                    val encryptedOutput = java.io.ByteArrayOutputStream()
                    CanonicalWorkspace(paired).download(ref.text("path")!!, encryptedOutput)
                    assertContentEquals(file.readBytes(), encryptedOutput.toByteArray())
                    client.call("RevokePairing", obj("pairId" to grant.field("peer").field("id")))
                    kotlinx.coroutines.withTimeout(10_000) { paired.awaitDisconnect() }
                    assertTrue(runCatching { paired.call("EngineInfo") }.isFailure)
                } finally {
                    paired.close()
                }
            } finally {
                file.delete()
                client.close()
            }
        }
}
