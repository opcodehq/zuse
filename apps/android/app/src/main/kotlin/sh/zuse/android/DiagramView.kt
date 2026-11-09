package sh.zuse.android

import android.annotation.SuppressLint
import android.content.ClipData
import android.content.ClipboardManager
import android.webkit.*
import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.serialization.encodeToString
import sh.zuse.core.WireJson

@Composable
fun DiagramView(source: String) {
    var expanded by remember { mutableStateOf(false) }
    val context = LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        DiagramSurface(source, Modifier.fillMaxWidth().height(280.dp))
        Row {
            TextButton(onClick = { expanded = true }) { Text("Expand diagram") }
            TextButton(
                onClick = {
                    context
                        .getSystemService(ClipboardManager::class.java)
                        .setPrimaryClip(ClipData.newPlainText("Diagram source", source))
                }
            ) {
                Text("Copy source")
            }
        }
    }
    if (expanded)
        Dialog(
            onDismissRequest = { expanded = false },
            properties = DialogProperties(usePlatformDefaultWidth = false),
        ) {
            Surface(Modifier.fillMaxSize()) {
                Column {
                    PaneHeader(
                        "Diagram",
                        actions = {
                            SmallIcon(Icons.Outlined.Close, "Close diagram", { expanded = false })
                        },
                    )
                    DiagramSurface(source, Modifier.weight(1f).fillMaxWidth())
                }
            }
        }
}

/**
 * Only this diagram surface uses WebView. Every request is intercepted; no network or JS bridge.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun DiagramSurface(source: String, modifier: Modifier) {
    var failed by remember(source) { mutableStateOf(false) }
    val currentSource by rememberUpdatedState(source)
    val dark = androidx.compose.foundation.isSystemInDarkTheme()
    val currentDark by rememberUpdatedState(dark)
    if (failed || source.length > 32768) {
        Text(source, style = MaterialTheme.typography.bodySmall, modifier = modifier)
        return
    }
    AndroidView(
        factory = { context ->
            WebView(context).apply {
                setBackgroundColor(android.graphics.Color.TRANSPARENT)
                settings.javaScriptEnabled = true
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                settings.domStorageEnabled = false
                settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                CookieManager.getInstance().setAcceptThirdPartyCookies(this, false)
                webViewClient =
                    object : WebViewClient() {
                        override fun shouldOverrideUrlLoading(
                            view: WebView,
                            request: WebResourceRequest,
                        ): Boolean = true

                        override fun shouldInterceptRequest(
                            view: WebView,
                            request: WebResourceRequest,
                        ): WebResourceResponse {
                            val url = request.url
                            return if (
                                url.scheme == "https" &&
                                    url.host == "appassets.androidplatform.net" &&
                                    url.path == "/assets/native/Mermaid.js"
                            )
                                WebResourceResponse(
                                    "application/javascript",
                                    "UTF-8",
                                    context.assets.open("native/Mermaid.js"),
                                )
                            else
                                WebResourceResponse(
                                    "text/plain",
                                    "UTF-8",
                                    403,
                                    "Blocked",
                                    emptyMap(),
                                    java.io.ByteArrayInputStream(ByteArray(0)),
                                )
                        }

                        override fun onPageFinished(view: WebView, url: String) {
                            view.evaluateJavascript(
                                "renderDiagram(${WireJson.encodeToString(currentSource)}, $currentDark)",
                                null,
                            )
                        }

                        override fun onRenderProcessGone(
                            view: WebView,
                            detail: RenderProcessGoneDetail,
                        ): Boolean {
                            failed = true
                            return true
                        }
                    }
                loadDataWithBaseURL(
                    "https://appassets.androidplatform.net/assets/native/index.html",
                    context.assets.open("native/Diagram.html").bufferedReader().use {
                        it.readText()
                    },
                    "text/html",
                    "UTF-8",
                    null,
                )
            }
        },
        update = { view ->
            view.evaluateJavascript(
                "if(typeof renderDiagram==='function')renderDiagram(${WireJson.encodeToString(source)}, $dark)",
                null,
            )
        },
        onRelease = { view ->
            view.stopLoading()
            view.webViewClient = WebViewClient()
            view.loadUrl("about:blank")
            view.destroy()
        },
        modifier = modifier,
    )
}
