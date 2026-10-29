package sh.zuse.core

import java.io.Closeable
import java.util.Base64
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import okhttp3.*
import okio.ByteString
import okio.ByteString.Companion.toByteString

/** A socket generation owns its Noise session. Text frames and plaintext fallback are rejected. */
class PairedSocket(
    private val link: String,
    private val http: OkHttpClient,
    private val receive: (String) -> Unit,
    private val failed: (Throwable) -> Unit,
) : WebSocketListener(), Closeable {
    private val lock = Any()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val ready = CompletableDeferred<Unit>()
    private var handle: Long? = null
    private var socket: WebSocket? = null
    private var handshake = false
    private var closed = false

    private fun request(op: String, vararg fields: Pair<String, JsonElement>): JsonObject =
        NativePairing.request(obj("op" to op.json(), "handle" to handle?.json(), *fields))

    private fun writeRecords(result: JsonElement) {
        result.array("records").forEach {
            val bytes = Base64.getDecoder().decode(it.jsonPrimitive.content)
            check(socket?.send(bytes.toByteString()) == true) { "Paired connection closed." }
        }
    }

    suspend fun connect() {
        try {
            synchronized(lock) {
                check(!closed && socket == null)
                val start =
                    NativePairing.request(obj("op" to "start".json(), "link" to link.json()))
                handle = requireNotNull(start.long("handle"))
                socket =
                    http.newWebSocket(
                        Request.Builder().url(requireNotNull(start.text("url"))).build(),
                        this,
                    )
                // onOpen sends the first handshake record after the HTTP upgrade.
                initial = start
            }
            withTimeout(10_000) { ready.await() }
        } catch (failure: Throwable) {
            close()
            throw failure
        }
    }

    private var initial: JsonObject? = null

    override fun onOpen(webSocket: WebSocket, response: Response) = guarded {
        writeRecords(requireNotNull(initial))
        initial = null
    }

    override fun onMessage(webSocket: WebSocket, bytes: ByteString) = guarded {
        require(bytes.size <= 65_535) { "Invalid paired record." }
        val result =
            request(if (handshake) "receive" else "finish", "record" to bytes.base64().json())
        if (!handshake) {
            handshake = true
            writeRecords(result)
            scope.launch {
                while (isActive) {
                    delay(1000)
                    guarded { writeRecords(request("poll")) }
                }
            }
        }
        if (result.flag("ready")) ready.complete(Unit)
        result.text("payload")?.let { encoded ->
            check(ready.isCompleted)
            receive(Base64.getDecoder().decode(encoded).toString(Charsets.UTF_8))
        }
    }

    fun send(text: String) = guarded {
        check(ready.isCompleted && !closed)
        writeRecords(
            request(
                "send",
                "payload" to
                    Base64.getEncoder().encodeToString(text.toByteArray(Charsets.UTF_8)).json(),
            )
        )
    }

    override fun onMessage(webSocket: WebSocket, text: String) = fail()

    override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) = fail()

    override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = fail()

    override fun onClosing(webSocket: WebSocket, code: Int, reason: String) = fail()

    private fun guarded(action: () -> Unit) {
        try {
            synchronized(lock) {
                check(!closed)
                action()
            }
        } catch (_: Throwable) {
            fail()
        }
    }

    private fun fail() {
        val error = ClientException("Paired connection lost. Reconnect with a valid invitation.")
        synchronized(lock) {
            if (closed) return
            ready.completeExceptionally(error)
            close()
        }
        failed(error)
    }

    override fun close() =
        synchronized(lock) {
            if (!closed) {
                closed = true
                ready.completeExceptionally(ClientException("Paired connection closed."))
                scope.cancel()
                socket?.cancel()
                handle?.let { runCatching { request("close") } }
                handle = null
                initial = null
            }
        }
}
