package sh.zuse.android

import android.graphics.Bitmap
import android.net.Uri
import androidx.activity.compose.setContent
import androidx.activity.findViewTreeOnBackPressedDispatcherOwner
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModelProvider
import java.io.File
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.annotation.LooperMode
import sh.zuse.core.*

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35], qualifiers = "w411dp-h914dp-xhdpi")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@LooperMode(LooperMode.Mode.PAUSED)
class NativeLayoutTests {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    private fun preview(large: Boolean = false) {
        compose.runOnUiThread {
            ViewModelProvider(compose.activity)[WorkspaceViewModel::class.java].handleLink(
                Uri.parse("zuse://preview?large=$large")
            )
        }
        compose.waitForIdle()
    }

    private fun screenshot(name: String) {
        val directory =
            File(requireNotNull(System.getProperty("zuse.screenshots"))).apply { mkdirs() }
        compose.waitForIdle()
        compose.runOnUiThread {
            val views =
                android.view.inspector.WindowInspector.getGlobalWindowViews().filter { it.isShown }
            val decor = compose.activity.window.decorView
            val bitmap = Bitmap.createBitmap(decor.width, decor.height, Bitmap.Config.ARGB_8888)
            val canvas = android.graphics.Canvas(bitmap)
            views.forEachIndexed { index, view ->
                if (index > 0) canvas.drawColor(0x33000000)
                view.draw(canvas)
            }
            File(directory, "$name.png").outputStream().use {
                bitmap.compress(Bitmap.CompressFormat.PNG, 100, it)
            }
        }
    }

    @Test
    fun phoneComposerAndTranscriptRender() {
        preview()
        compose.onNodeWithContentDescription("Ask Zuse").assertIsDisplayed()
        compose.onNodeWithContentDescription("Chat transcript").assertIsDisplayed()
        val composer = compose.onNodeWithContentDescription("Message composer")
        composer.assert(
            SemanticsMatcher.expectValue(
                androidx.compose.ui.semantics.SemanticsProperties.StateDescription,
                "Collapsed",
            )
        )
        val resting = composer.fetchSemanticsNode().boundsInRoot.height
        org.junit.Assert.assertEquals(
            44f * compose.activity.resources.displayMetrics.density,
            resting,
            2f,
        )
        screenshot("phone-chat-collapsed")
        compose.onNodeWithContentDescription("Ask Zuse").performClick()
        composer.assert(
            SemanticsMatcher.expectValue(
                androidx.compose.ui.semantics.SemanticsProperties.StateDescription,
                "Expanded",
            )
        )
        compose
            .onNodeWithContentDescription("Ask Zuse")
            .performTextInput("Make the composer feel like the mobile app.")
        org.junit.Assert.assertTrue(composer.fetchSemanticsNode().boundsInRoot.height > resting * 2)
        screenshot("phone-chat-expanded")
        compose.onNodeWithContentDescription("Ask Zuse").performTextReplacement("")
        compose.activityRule.scenario.onActivity { it.onBackPressedDispatcher.onBackPressed() }
        composer.assert(
            SemanticsMatcher.expectValue(
                androidx.compose.ui.semantics.SemanticsProperties.StateDescription,
                "Collapsed",
            )
        )
        screenshot("phone-chat")
    }

    @Test
    fun restingComposerAccommodatesLargeAccessibilityType() {
        compose.runOnUiThread {
            compose.activity.setContent {
                val density = androidx.compose.ui.platform.LocalDensity.current
                androidx.compose.runtime.CompositionLocalProvider(
                    androidx.compose.ui.platform.LocalDensity provides
                        androidx.compose.ui.unit.Density(density.density, 2f)
                ) {
                    ZuseTheme {
                        Surface(Modifier.fillMaxSize()) {
                            Column(Modifier.padding(12.dp)) {
                                ComposerFrame(
                                    "",
                                    {},
                                    expanded = false,
                                    leadingAction = {},
                                    trailingAction = {},
                                )
                            }
                        }
                    }
                }
            }
        }
        val frame =
            compose
                .onNodeWithContentDescription("Message composer")
                .fetchSemanticsNode()
                .boundsInRoot
        val placeholder = compose.onNodeWithText("Ask Zuse").fetchSemanticsNode().boundsInRoot
        org.junit.Assert.assertTrue(
            placeholder.top >= frame.top && placeholder.bottom <= frame.bottom
        )
    }

    @Test
    fun nativeBackNavigatesSettingsBeforeDismissing() {
        compose.onNodeWithContentDescription("Settings").performClick()
        compose.onNodeWithText("Computers").performClick()
        compose.onNodeWithText("Pair a computer").assertExists()
        compose.runOnUiThread {
            val window =
                android.view.inspector.WindowInspector.getGlobalWindowViews().last { it.isShown }
            requireNotNull(window.findViewTreeOnBackPressedDispatcherOwner())
                .onBackPressedDispatcher
                .onBackPressed()
        }
        compose.onNodeWithText("Pair a computer").assertDoesNotExist()
        compose.onNodeWithText("Zuse Cloud").assertExists()
    }

    @Test
    fun unfinishedNewChatDraftSurvivesClosingAndReopening() {
        preview()
        compose.onNodeWithContentDescription("Chat actions").performClick()
        compose.onNodeWithText("New chat").performClick()
        compose
            .onNodeWithContentDescription("New chat message")
            .performTextInput("Keep this new chat draft")
        compose.onNodeWithContentDescription("Close new chat").performClick()
        compose.onNodeWithContentDescription("Chat actions").performClick()
        compose.onNodeWithText("New chat").performClick()
        compose
            .onNodeWithContentDescription("New chat message")
            .assertTextContains("Keep this new chat draft")
    }

    @Test
    @Config(qualifiers = "w900dp-h700dp-land-xhdpi")
    fun tabletRetainsListBesideConversation() {
        preview()
        compose.onNodeWithContentDescription("Ask Zuse").assertIsDisplayed()
        compose.onNodeWithContentDescription("Settings").assertIsDisplayed()
        compose.onNodeWithContentDescription("Chat transcript").assertIsDisplayed()
        compose.onNodeWithContentDescription("Back to chats").assertDoesNotExist()
        screenshot("tablet-chat")
        compose.onNodeWithContentDescription("Settings").performClick()
        screenshot("tablet-settings")
    }

    @Test
    @Config(qualifiers = "w841dp-h701dp-land-xhdpi")
    fun foldSizedWindowKeepsListAndConversation() {
        preview()
        compose.onNodeWithContentDescription("Settings").assertExists()
        compose.onNodeWithContentDescription("Back to chats").assertDoesNotExist()
        compose.onNodeWithContentDescription("Ask Zuse").assertIsDisplayed()
        screenshot("fold-open-chat")
    }

    @Test
    @Config(qualifiers = "w411dp-h914dp-night-xhdpi")
    fun darkModeKeepsComposerReadable() {
        preview()
        compose.onNodeWithContentDescription("Ask Zuse").assertIsDisplayed()
        screenshot("phone-chat-dark")
    }

    @Test
    fun tenThousandMessageTranscriptScrollsBothDirections() {
        preview(large = true)
        compose.onNodeWithContentDescription("Chat transcript").performScrollToIndex(0)
        compose.onNodeWithContentDescription("Chat transcript").performScrollToIndex(9999)
        compose.onNodeWithContentDescription("Ask Zuse").assertIsDisplayed()
    }

    @Test
    fun nativeMathProducesAnEquationDrawable() {
        val message =
            Message(
                "math",
                "preview-session",
                "assistant",
                obj(
                    "_tag" to "assistant".json(),
                    "text" to "Energy, in native math:\n\n\$\$\nE = mc^2\n\$\$".json(),
                ),
            )
        compose.runOnUiThread {
            compose.activity.setContent {
                ZuseTheme {
                    Surface(Modifier.fillMaxSize()) {
                        Column(Modifier.padding(20.dp)) { MessageRow(message) }
                    }
                }
            }
        }
        screenshot("native-math-initial")
        try {
            compose.waitUntil(5000) {
                org.robolectric.Shadows.shadowOf(android.os.Looper.getMainLooper()).idle()
                var rendered = false
                compose.runOnUiThread {
                    fun visit(view: android.view.View) {
                        if (view is android.widget.TextView) {
                            val text = view.text as? android.text.Spanned
                            if (
                                text != null &&
                                    text
                                        .getSpans(
                                            0,
                                            text.length,
                                            io.noties.markwon.image.AsyncDrawableSpan::class.java,
                                        )
                                        .any { it.drawable.hasResult() }
                            )
                                rendered = true
                        }
                        if (view is android.view.ViewGroup)
                            for (index in 0 until view.childCount) visit(view.getChildAt(index))
                    }
                    visit(compose.activity.window.decorView)
                }
                rendered
            }
        } catch (failure: Throwable) {
            org.robolectric.shadows.ShadowLog.getLogsForTag("JLatexMathPlugin").forEach { item ->
                println(item.msg)
                item.throwable?.printStackTrace()
            }
            throw failure
        }
        screenshot("native-math")
    }

    @Test
    fun generatedChartsAndFormsRenderAndSubmitTypedValues() {
        val fixture =
            compose.activity.assets.open("native/visual-preview.json").bufferedReader().use {
                WireJson.parseToJsonElement(it.readText())
            }
        var response: String? = null
        val drafts = mutableMapOf<String, JsonElement>()
        fun show(version: Int) {
            compose.runOnUiThread {
                compose.activity.setContent {
                    ZuseTheme {
                        Surface(Modifier.fillMaxSize()) {
                            androidx.compose.runtime.key(version) {
                                Column(
                                    Modifier.verticalScroll(rememberScrollState()).padding(20.dp)
                                ) {
                                    UiSpecView(
                                        fixture.text("spec").orEmpty(),
                                        respond = { response = it },
                                        validatedPreview = fixture.field("root"),
                                        savedDraft = drafts.toMap(),
                                        onDraftChange = { name, value -> drafts[name] = value },
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
        show(0)
        compose.onNodeWithText("Workspace health").assertIsDisplayed()
        compose.onNodeWithText("feature/native-android").performTextReplacement("feature/foldable")
        org.junit.Assert.assertEquals(
            "feature/foldable",
            (drafts["branch"] as? JsonPrimitive)?.content,
        )
        show(1)
        compose.onNodeWithText("feature/foldable").assertExists()
        screenshot("generated-visual")
        compose.onNodeWithText("Run workspace checks").performScrollTo().performClick()
        compose.waitUntil(5000) { response != null }
        org.junit.Assert.assertTrue(response!!.contains("feature/foldable"))
        org.junit.Assert.assertTrue(response!!.contains("true"))
        compose.onNodeWithText("Response queued").assertExists()
    }

    @Test
    fun settingsExposeCloudPairingAndTailscale() {
        compose.runOnUiThread {
            ViewModelProvider(compose.activity)[WorkspaceViewModel::class.java]
                .devices
                .addAll(
                    listOf(
                        SavedDevice(
                            "demo-mac",
                            "MacBook Pro · demo",
                            "wss://macbook.example/rpc",
                            "",
                        ),
                        SavedDevice(
                            "demo-server",
                            "Home server · demo",
                            "wss://server.example/rpc",
                            "",
                        ),
                    )
                )
        }
        compose.onNodeWithContentDescription("Settings").performClick()
        compose.onNodeWithText("Zuse Cloud").assertExists()
        screenshot("phone-settings")
        compose.onNodeWithText("Computers").performClick()
        compose.onNodeWithText("Pair a computer").assertExists()
        screenshot("phone-computers")
        compose.onNodeWithText("Pair a computer").performClick()
        compose.onNodeWithText("Pairing link").assertExists()
        screenshot("phone-pairing")
        compose.onNodeWithContentDescription("Back to settings").performClick()
        compose.onNodeWithText("Add manually").performClick()
        compose.onNodeWithText("WebSocket address").assertExists()
        screenshot("phone-connections")
        compose.onNodeWithContentDescription("Back to settings").performClick()
        compose.onNodeWithContentDescription("Back to settings").performClick()
        compose.onNodeWithText("Sign in").performClick()
        screenshot("phone-account")
    }

    @Test
    fun phoneHomeShowsCompactChatRowsAndComputerNames() {
        preview()
        val workspace = ViewModelProvider(compose.activity)[WorkspaceViewModel::class.java]
        compose.runOnUiThread {
            listOf(
                    "Review the Android composer",
                    "Fix reconnect recovery",
                    "Add tablet navigation",
                    "Polish connection settings",
                )
                .forEachIndexed { index, title ->
                    workspace.sessions.add(
                        Session("demo-$index", "preview-project", title, "claude", "Sonnet · demo")
                    )
                }
        }
        compose.onNodeWithContentDescription("Back to chats").performClick()
        compose.onNodeWithText("Your workspace").assertDoesNotExist()
        screenshot("phone-home")
    }

    @Test
    fun changesRenderScopesAndNativePatchLines() {
        preview()
        val workspace = ViewModelProvider(compose.activity)[WorkspaceViewModel::class.java]
        compose.runOnUiThread {
            compose.activity.setContent {
                ZuseTheme { Surface(Modifier.fillMaxSize()) { DiffSheet(workspace, {}) } }
            }
        }
        compose.waitForIdle()
        compose.runOnUiThread {
            workspace.patches.add(
                Patch(
                    "app/Composer.kt",
                    DiffLine.parse(
                        "@@ -12,3 +12,4 @@\n val expanded = focused\n-Composer(height = 108.dp)\n+Composer(height = if (expanded) 96.dp else 44.dp)\n+ModelPicker(selectedModel)\n showKeyboard()"
                    ),
                    "text",
                    false,
                    null,
                )
            )
        }
        compose.onNodeWithText("Working tree").assertExists()
        compose.onNodeWithText("app/Composer.kt").assertExists()
        screenshot("phone-changes")
    }

    @Test
    fun newChatShowsProjectBranchAndModelChoices() {
        preview()
        val workspace = ViewModelProvider(compose.activity)[WorkspaceViewModel::class.java]
        compose.runOnUiThread {
            workspace.models.addAll(
                listOf(
                    ModelChoice("claude", "sonnet-demo", "Sonnet · demo"),
                    ModelChoice("codex", "codex-demo", "Codex · demo"),
                )
            )
            compose.activity.setContent {
                ZuseTheme { Surface(Modifier.fillMaxSize()) { NewChatSheet(workspace, {}, {}) } }
            }
        }
        compose.onNodeWithText("Where").assertExists()
        compose.onNodeWithText("Project").assertExists()
        compose.onNodeWithContentDescription("New chat message").assertExists()
        compose.onNodeWithText("Mode").assertExists()
        screenshot("phone-new-chat")
        compose.onNodeWithText("Sonnet · demo").performClick()
        compose.onNodeWithText("Codex · demo").assertExists()
        screenshot("phone-model-picker")
    }
}
