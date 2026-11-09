package sh.zuse.android

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.ViewModelProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.UUID
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import sh.zuse.core.*

/** Actual Compose UI + packaged JNI + canonical engine, with an isolated deterministic agent. */
@RunWith(AndroidJUnit4::class)
class RealHostUiTests {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    @Test
    fun composerAndQuestionAnswersReachTheEncryptedRustHost() = runBlocking {
        val filename = InstrumentationRegistry.getArguments().getString("hostConfig")
        assumeTrue("Run scripts/test-real-engine.py --device", filename != null)
        require(filename!!.matches(Regex("host-test-[a-f0-9]+\\.json")))
        val config =
            WireJson.parseToJsonElement(File(compose.activity.filesDir, filename).readText())
        val address = requireNotNull(config.text("address"))
        require(java.net.URI(address).host == "127.0.0.1")
        val owner = RpcClient(address, token = config.text("token"))
        lateinit var model: WorkspaceViewModel
        var modelReady = false
        var pairId: JsonElement? = null
        try {
            owner.connect()
            val id = UUID.randomUUID().toString()
            val host = CanonicalWorkspace(owner)
            host.create(
                obj(
                    "operationId" to id.json(),
                    "projectId" to CanonicalWorkspace.PROJECTLESS.json(),
                    "providerId" to "mock".json(),
                    "model" to "mock-1".json(),
                ),
                null,
                false,
            )
            host.rename(id, "Android encrypted UI test")
            val grant = owner.call("CreatePairing", obj("label" to "Android UI acceptance".json()))
            pairId = grant.field("peer").field("id")
            val link =
                "http://${java.net.URI(address).rawAuthority}/${requireNotNull(grant.text("fragment"))}"
            compose.runOnUiThread {
                model = ViewModelProvider(compose.activity)[WorkspaceViewModel::class.java]
                modelReady = true
                model.connect(
                    Source.Device(
                        SavedDevice(
                            UUID.randomUUID().toString(),
                            "Isolated Rust host",
                            address,
                            "",
                            pairingLink = link,
                        )
                    )
                )
            }
            compose.waitUntil(30_000) {
                compose
                    .onAllNodesWithText("Android encrypted UI test")
                    .fetchSemanticsNodes()
                    .isNotEmpty()
            }
            compose.onNodeWithText("Android encrypted UI test").performClick()
            compose.waitUntil(15_000) { model.synced }
            compose
                .onNodeWithContentDescription("Ask Zuse")
                .performTextInput("Verify Android remote UI")
            compose.onNodeWithContentDescription("Send message").performClick()
            compose.waitUntil(15_000) {
                compose
                    .onAllNodesWithText("Which sync strategy should the rewrite use?")
                    .fetchSemanticsNodes()
                    .isNotEmpty()
            }
            compose.onNodeWithText("Poll the doc host every 120ms").performScrollTo().performClick()
            compose.onNodeWithText("Unit tests").performScrollTo().performClick()
            compose.onNodeWithText("Submit answers").performScrollTo().performClick()
            compose.waitUntil(15_000) {
                model.messages.any { it.content.text("text")?.contains("Locked in:") == true }
            }
            compose.waitUntil(5000) {
                fun rendered(view: android.view.View): Boolean =
                    (view is android.widget.TextView &&
                        view.text.toString().contains("Locked in:")) ||
                        (view is android.view.ViewGroup &&
                            (0 until view.childCount).any { rendered(view.getChildAt(it)) })
                var found = false
                compose.runOnUiThread { found = rendered(compose.activity.window.decorView) }
                found
            }
            val screenshot =
                requireNotNull(
                    InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
                )
            File(compose.activity.filesDir, "real-host-chat.png").outputStream().use {
                screenshot.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
            }
            screenshot.recycle()
        } finally {
            compose.runOnUiThread { if (modelReady) model.disconnect() }
            pairId?.let { runCatching { owner.call("RevokePairing", obj("pairId" to it)) } }
            owner.close()
        }
    }
}
