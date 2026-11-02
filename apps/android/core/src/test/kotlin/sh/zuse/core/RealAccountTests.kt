package sh.zuse.core

import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Started by the Rust managed transport acceptance test with an isolated authority. */
class RealAccountTests {
    @Test
    fun `native account client authenticates renews and cancels real host stream`() = runBlocking {
        val origin = System.getenv("ZUSE_NATIVE_ACCOUNT_ORIGIN")
        assumeTrue("Requires the isolated Rust managed transport harness", origin != null)
        require(java.net.URI(origin).host == "127.0.0.1")
        val grants = AtomicInteger()
        val client =
            RpcClient(
                origin!!,
                accountEnvironmentId = "host-device",
                accountGrant = {
                    val initial = grants.getAndIncrement() == 0
                    AccountGrant(
                        "host-device",
                        origin,
                        if (initial) "renewable-grant" else "renewed-grant",
                        System.currentTimeMillis() + if (initial) 3000 else 60_000,
                    )
                },
            )
        try {
            client.connect()
            assertEquals("host-device", client.call("EngineInfo").text("deviceId"))
            assertTrue(client.stream("Watch").first().flag("ready"))
            withTimeout(5000) { while (grants.get() < 2) delay(50) }
            // Continue beyond the original grant's maximum lifetime.
            delay(3000)
            assertEquals("host-device", client.call("EngineInfo").text("deviceId"))
        } finally {
            client.close()
        }
    }
}
