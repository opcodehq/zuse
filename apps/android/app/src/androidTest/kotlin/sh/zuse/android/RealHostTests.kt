package sh.zuse.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.ByteArrayOutputStream
import java.io.File
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import sh.zuse.core.*

/** Opt-in isolated host acceptance; the launcher puts credentials in the app's private files. */
@RunWith(AndroidJUnit4::class)
class RealHostTests {
    @Test
    fun packagedClientPairsTransfersAndObservesRevocation() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val filename = InstrumentationRegistry.getArguments().getString("hostConfig")
        assumeTrue("Run scripts/test-real-engine.py --device for live acceptance", filename != null)
        require(filename!!.matches(Regex("host-test-[a-f0-9]+\\.json")))
        val context = instrumentation.targetContext
        val config = Json.parseToJsonElement(File(context.filesDir, filename).readText())
        val address = requireNotNull(config.text("address"))
        require(java.net.URI(address).host == "127.0.0.1")
        val owner = RpcClient(address, token = config.text("token"))
        val file = File.createTempFile("real-host", ".txt", context.cacheDir)
        try {
            owner.connect()
            val identity = owner.call("EngineInfo").text("deviceId")
            assertNotNull(identity)
            val grant =
                owner.call("CreatePairing", obj("label" to "Android emulator acceptance".json()))
            val link =
                "http://${java.net.URI(address).rawAuthority}/${requireNotNull(grant.text("fragment"))}"
            val paired = RpcClient(address, pairingLink = link)
            try {
                paired.connect()
                assertEquals(identity, paired.call("EngineInfo").text("deviceId"))
                val workspace = CanonicalWorkspace(paired)
                assertTrue(workspace.projects().any { it.id == CanonicalWorkspace.PROJECTLESS })
                // Crosses multiple upload chunks and encrypted record boundaries.
                file.writeText("Android encrypted transfer · 🌍\n".repeat(6000))
                val attachment = workspace.upload(file, "device.txt", "text/plain")
                val output = ByteArrayOutputStream()
                workspace.download(requireNotNull(attachment.text("path")), output)
                assertArrayEquals(file.readBytes(), output.toByteArray())
                owner.call("RevokePairing", obj("pairId" to grant.field("peer").field("id")))
                withTimeout(10_000) { paired.awaitDisconnect() }
                assertTrue(runCatching { paired.call("EngineInfo") }.isFailure)
            } finally {
                paired.close()
            }
        } finally {
            owner.close()
            file.delete()
        }
    }
}
