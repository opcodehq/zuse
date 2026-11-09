package sh.zuse.core

import kotlin.test.*
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.junit.Assume.assumeTrue
import org.junit.Test

class RealCloudTests {
    @Test
    fun `native client uses real Rust gateway routing and guest RPC`() = runBlocking {
        val address = System.getenv("ZUSE_NATIVE_CLOUD_ADDRESS")
        assumeTrue("Requires isolated Rust gateway acceptance", address != null)
        require(java.net.URI(address).host == "127.0.0.1")
        val client =
            RpcClient(
                address!!,
                listOf(CloudGateway.RUST, "client-fixture"),
                cloudExpiresAt = System.getenv("ZUSE_NATIVE_CLOUD_EXPIRY").toLong(),
            )
        try {
            client.connect()
            assertEquals("native-cloud-guest", client.call("EngineInfo").text("deviceId"))
            val text = "Native cloud round trip · 🌍\n".repeat(5000)
            assertEquals(text, client.call("Echo", obj("text" to text.json())).text("text"))
            assertTrue(client.stream("Watch").first().flag("ready"))
            assertEquals(
                "after-cancel",
                client.call("Echo", obj("text" to "after-cancel".json())).text("text"),
            )
        } finally {
            client.close()
        }
    }
}
