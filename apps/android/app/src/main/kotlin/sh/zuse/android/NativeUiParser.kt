package sh.zuse.android

import android.content.Context
import android.util.LruCache
import androidx.javascriptengine.IsolateStartupParameters
import androidx.javascriptengine.JavaScriptIsolate
import androidx.javascriptengine.JavaScriptSandbox
import kotlinx.coroutines.*
import kotlinx.coroutines.guava.await
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.JsonElement
import sh.zuse.core.*

/** The shared validated OpenUI parser runs without DOM, network, or Android/credential access. */
class NativeUiParser(private val context: Context) {
    private val lock = Mutex()
    private val cache = LruCache<String, JsonElement>(16)
    private var sandbox: JavaScriptSandbox? = null
    private var isolate: JavaScriptIsolate? = null

    suspend fun parse(spec: String): JsonElement =
        withContext(Dispatchers.Default) {
            require(spec.length <= 32 * 1024) { "This visual exceeds the supported size." }
            lock.withLock {
                cache.get(spec)?.let {
                    return@withLock it
                }
                try {
                    var engine = isolate
                    if (engine == null) {
                        if (!JavaScriptSandbox.isSupported())
                            throw ClientException(
                                "Update Android System WebView to render this visual. Its source remains available below."
                            )
                        val process =
                            withTimeout(10_000) {
                                JavaScriptSandbox.createConnectedInstanceAsync(
                                        context.applicationContext
                                    )
                                    .await()
                            }
                        sandbox = process
                        if (
                            !process.isFeatureSupported(
                                JavaScriptSandbox.JS_FEATURE_ISOLATE_MAX_HEAP_SIZE
                            )
                        ) {
                            throw ClientException(
                                "This device cannot provide a bounded visual parser. Update Android System WebView."
                            )
                        }
                        engine =
                            process.createIsolate(
                                IsolateStartupParameters().apply {
                                    setMaxHeapSizeBytes(64L * 1024 * 1024)
                                }
                            )
                        isolate = engine
                        val script =
                            withContext(Dispatchers.IO) {
                                context.assets.open("native/UIParser.js").bufferedReader().use {
                                    it.readText()
                                }
                            }
                        withTimeout(5000) { engine.evaluateJavaScriptAsync(script).await() }
                    }
                    val result =
                        withTimeout(2000) {
                            engine
                                .evaluateJavaScriptAsync(
                                    "parseNativeSpec(${WireJson.encodeToString(spec)})"
                                )
                                .await()
                        }
                    require(result.length <= 1024 * 1024) {
                        "This visual produced too much content."
                    }
                    val root = WireJson.parseToJsonElement(result)
                    validateUiTree(root)
                    cache.put(spec, root)
                    root
                } catch (_: TimeoutCancellationException) {
                    close()
                    throw ClientException(
                        "This visual took too long to render. Its source remains available below."
                    )
                } catch (error: Throwable) {
                    close()
                    throw error
                }
            }
        }

    fun close() {
        isolate?.close()
        isolate = null
        sandbox?.close()
        sandbox = null
    }
}
