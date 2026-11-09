package sh.zuse.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.net.Socket
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import sh.zuse.core.*

/**
 * Real routes in the packaged app: real sockets, JNI and engine. Run through
 * scripts/verify-routes.py, which starts the host, relay and a killable proxy.
 */
@RunWith(AndroidJUnit4::class)
class RemoteRoutesTests {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private lateinit var config: JsonElement
    private lateinit var owner: RpcClient
    private val opened = mutableListOf<RpcClient>()

    private fun setting(name: String) = requireNotNull(config.text(name)) { "missing $name" }

    @Before
    fun connectOwner() = runBlocking {
        val filename = InstrumentationRegistry.getArguments().getString("hostConfig")
        assumeTrue("Run scripts/verify-routes.py", filename != null)
        require(filename!!.matches(Regex("host-test-[a-f0-9]+\\.json")))
        config = Json.parseToJsonElement(File(context.filesDir, filename).readText())
        PairedNetwork.routesDirectory = null
        owner = RpcClient(setting("address"), token = setting("token"))
        owner.connect()
    }

    @After
    fun closeClients() {
        opened.forEach { it.close() }
        if (::owner.isInitialized) owner.close()
        shell("cmd connectivity airplane-mode disable")
    }

    private fun shell(command: String) {
        instrumentation.uiAutomation.executeShellCommand(command).close()
    }

    private suspend fun grant(): JsonElement =
        owner.call("CreatePairing", obj("label" to "Route check".json()))

    private inner class Link(origin: String, fragment: String) {
        val route = MutableStateFlow<PairedRouteStatus?>(null)
        val client =
            RpcClient(origin, pairingLink = "$origin/$fragment", onRoute = { route.value = it })
                .also { opened.add(it) }

        suspend fun awaitRoute(label: String, seconds: Long = 30): PairedRouteStatus =
            withTimeout(seconds * 1000) {
                route.filterNotNull().first { it.state == "connected" && it.label == label }
            }

        suspend fun assertHostAnswers() {
            assertNotNull(client.call("EngineInfo").text("deviceId"))
        }
    }

    private suspend fun link(origin: String): Link {
        val link = Link(origin, requireNotNull(grant().text("fragment")))
        link.client.connect()
        return link
    }

    private fun report(name: String, status: PairedRouteStatus?) {
        android.util.Log.i("ZuseRoutes", "$name: ${status?.label} rtt=${status?.rttMs}ms")
    }

    @Test
    fun connectsOverTheLocalNetwork() = runBlocking {
        val link = link(setting("lan"))
        val status = link.awaitRoute("Local network")
        assertTrue(status.resumable)
        link.assertHostAnswers()
        report("lan", status)
    }

    @Test
    fun connectsOverTheTailnetAddress() = runBlocking {
        val fragment = requireNotNull(grant().text("fragment"))
        val link = Link(setting("tailnet"), fragment)
        link.client.connect()
        assertTrue(link.route.value?.label in listOf("Tailscale", "Local network"))
        link.assertHostAnswers()
        report("tailnet", link.route.value)
    }

    @Test
    fun movesFromTheTailnetToTheLocalNetworkWithoutDisconnecting() = runBlocking {
        val link = link(setting("tailnet"))
        var disconnected = false
        val watcher = launch {
            link.client.awaitDisconnect()
            disconnected = true
        }
        // The route starts on the tailnet and a learned LAN route wins two probes.
        val status = link.awaitRoute("Local network", 30)
        link.assertHostAnswers()
        assertFalse("the move must not end the session", disconnected)
        watcher.cancel()
        report("upgrade", status)
    }

    @Test
    fun connectsThroughARealRelay() = runBlocking {
        // A relay on this machine's loopback is reached through adb reverse.
        val fragment = requireNotNull(grant().text("fragment"))
        // A new invitation takes a moment to register with the relay.
        val link =
            withTimeout(15_000) {
                var connected: Link? = null
                while (connected == null) {
                    val attempt = Link(setting("relay"), fragment)
                    if (runCatching { attempt.client.connect() }.isSuccess) connected = attempt
                    else delay(500)
                }
                connected
            }
        link.assertHostAnswers()
        report("relay", link.route.value)
    }

    @Test
    fun aKilledRouteResumesElsewhereAndKeepsTheSession() = runBlocking {
        val link = link(setting("proxy"))
        link.assertHostAnswers()
        val first = link.route.value
        var disconnected = false
        val watcher = launch {
            link.client.awaitDisconnect()
            disconnected = true
        }
        withContext(Dispatchers.IO) {
            Socket(setting("proxyControlHost"), setting("proxyControlPort").toInt()).use {
                it.getOutputStream().write(setting("proxySecret").toByteArray())
            }
        }
        // Sent into the outage: it must arrive after the session moves.
        link.assertHostAnswers()
        assertFalse(disconnected)
        watcher.cancel()
        report("resume", link.route.value)
        assertNotNull(first)
        assertTrue(link.route.value?.resumable == true)
    }

    @Test
    fun rememberedRoutesReachAHostWhoseInvitationAddressIsGone() = runBlocking {
        val directory = File(context.cacheDir, "routes-${System.nanoTime()}").apply { mkdirs() }
        val fragment = requireNotNull(grant().text("fragment"))
        val pairId = Invitation.idOf(fragment)
        File(directory, "$pairId.json")
            .writeText(
                obj(
                        "learned" to
                            JsonArray(
                                listOf(
                                    obj("kind" to "lan".json(), "origin" to setting("lan").json())
                                )
                            ),
                        "bestByNetwork" to obj(),
                    )
                    .toString()
            )
        PairedNetwork.routesDirectory = directory.path
        val link = Link(setting("deadOrigin"), fragment)
        link.client.connect()
        link.assertHostAnswers()
        report("remembered", link.route.value)
    }

    @Test
    fun anAirplaneModeDropResumesTheSameSession() = runBlocking {
        val link = link(setting("lan"))
        link.assertHostAnswers()
        var disconnected = false
        val watcher = launch {
            link.client.awaitDisconnect()
            disconnected = true
        }
        shell("cmd connectivity airplane-mode enable")
        delay(8_000)
        shell("cmd connectivity airplane-mode disable")
        withTimeout(40_000) {
            while (true) {
                if (link.route.value?.state == "connected") break
                delay(250)
            }
            link.assertHostAnswers()
        }
        assertFalse("the drop must not end the session", disconnected)
        watcher.cancel()
        report("airplane", link.route.value)
    }

    @Test
    fun revokingAPairingEndsTheConnectionPromptly() = runBlocking {
        val created = grant()
        val link = Link(setting("lan"), requireNotNull(created.text("fragment")))
        link.client.connect()
        link.assertHostAnswers()
        owner.call("RevokePairing", obj("pairId" to created.field("peer").field("id")))
        withTimeout(10_000) { link.client.awaitDisconnect() }
        assertTrue(runCatching { link.client.call("EngineInfo") }.isFailure)
    }

    private object Invitation {
        fun idOf(fragment: String): String {
            val encoded = fragment.substringAfter("pair=").substringBefore("&")
            val wire = java.util.Base64.getUrlDecoder().decode(encoded)
            return requireNotNull(Json.parseToJsonElement(String(wire)).text("id"))
        }
    }
}
