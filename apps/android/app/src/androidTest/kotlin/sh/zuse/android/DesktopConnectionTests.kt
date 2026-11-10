package sh.zuse.android

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.ViewModelProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.UUID
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import sh.zuse.core.*

/** Opt-in acceptance against the running headed desktop. Keeps the pairing for manual use. */
@RunWith(AndroidJUnit4::class)
class DesktopConnectionTests {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    @Test
    fun stagingPairsWithTheRunningDesktopAndPersistsTheComputer() = runBlocking {
        val filename = InstrumentationRegistry.getArguments().getString("desktopConfig")
        assumeTrue("Run scripts/connect-local-desktop.py", filename != null)
        require(BuildConfig.DEPLOYMENT_PROFILE == "staging")
        require(filename!!.matches(Regex("desktop-test-[a-f0-9]+\\.json")))
        val config = WireJson.parseToJsonElement(File(compose.activity.filesDir, filename).readText())
        val origin = requireNotNull(config.text("origin"))
        require(java.net.URI(origin).host == "127.0.0.1")
        val link = requireNotNull(config.text("link"))
        val address = origin.replaceFirst("http://", "ws://") + "/api/rpc"
        val paired = RpcClient(address, pairingLink = link)
        val chatId =
            UUID.nameUUIDFromBytes(("staging-android:" + config.text("deviceId")).toByteArray())
                .toString()
        try {
            paired.connect()
            assertEquals(config.text("deviceId"), paired.call("EngineInfo").text("deviceId"))
            val host = CanonicalWorkspace(paired)
            assertTrue(host.projects().isNotEmpty())
            host.create(
                obj(
                    "operationId" to chatId.json(),
                    "projectId" to requireNotNull(config.text("projectId")).json(),
                    "providerId" to "codex".json(),
                    "model" to "gpt-6.1-sol".json(),
                ),
                null,
                false,
            )
            host.rename(chatId, "Android ↔ Rust staging")
        } finally {
            paired.close()
        }
        lateinit var model: WorkspaceViewModel
        var saved = false
        compose.runOnUiThread {
            model = ViewModelProvider(compose.activity)[WorkspaceViewModel::class.java]
            model.pair(link, "Local Rust staging") { saved = true }
        }
        compose.waitUntil(30_000) { saved && model.connected }
        assertTrue(model.devices.any { it.label == "Local Rust staging" && it.pairingLink == link })
        assertNull(model.error)
        // A new vault reader proves this is durable rather than an in-memory UI fixture.
        val persisted = Vault(compose.activity).read("devices")
        assertTrue(persisted?.contains("Local Rust staging") == true)
        compose.waitUntil(30_000) {
            compose.onAllNodesWithText("Android ↔ Rust staging").fetchSemanticsNodes().isNotEmpty()
        }
        compose.onNodeWithText("Android ↔ Rust staging").performClick()
        compose.waitUntil(30_000) { model.selectedSession?.id == chatId && model.synced }
        if (InstrumentationRegistry.getArguments().getString("browseFiles") == "true") {
            compose.onNodeWithContentDescription("Chat actions").performClick()
            compose.onNodeWithText("Review changes").performClick()
            compose.onNodeWithContentDescription("Browse all files").performClick()
            compose.waitUntil(30_000) { compose.onAllNodesWithText("apps").fetchSemanticsNodes().isNotEmpty() }
            compose.onNodeWithContentDescription("Workspace files").performScrollToNode(hasText("apps"))
            compose.onNodeWithText("apps").performClick()
            compose.waitUntil(30_000) { compose.onAllNodesWithText("android").fetchSemanticsNodes().isNotEmpty() }
            compose.onNodeWithContentDescription("Parent folder").performClick()
            compose.waitUntil(30_000) { compose.onAllNodesWithText("Project root").fetchSemanticsNodes().isNotEmpty() }
            compose.waitUntil(30_000) { compose.onAllNodesWithText("apps").fetchSemanticsNodes().isNotEmpty() }
            compose.onNodeWithContentDescription("Workspace files").performScrollToNode(hasText("Cargo.toml"))
            compose.onNodeWithText("Cargo.toml").performClick()
            compose.waitUntil(30_000) { compose.onAllNodesWithText("[workspace]", substring = true).fetchSemanticsNodes().isNotEmpty() }
        }
        if (InstrumentationRegistry.getArguments().getString("realAgent") == "true") {
            val expected = "Android desktop connection works."
            val previous = model.messages.filter { it.role == "assistant" }.map { it.id }.toSet()
            compose.onNodeWithContentDescription("Ask Zuse").performTextInput(
                "Reply with exactly: $expected Do not use tools, run commands, or edit files."
            )
            compose.onNodeWithContentDescription("Send message").performClick()
            compose.waitUntil(120_000) {
                model.messages.any {
                    it.role == "assistant" && it.id !in previous &&
                        it.content.text("text")?.contains(expected) == true
                }
            }
        }
    }
}
