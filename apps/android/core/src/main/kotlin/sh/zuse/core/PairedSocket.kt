package sh.zuse.core

import java.io.Closeable
import java.util.Base64
import java.util.concurrent.CopyOnWriteArraySet
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import okhttp3.*
import okio.ByteString
import okio.ByteString.Companion.toByteString

/** What the interface shows about a paired connection. Contains no secrets. */
data class PairedRouteStatus(
    val state: String,
    val label: String?,
    val rttMs: Long?,
    val resumable: Boolean,
)

/** Lets the app tell live paired connections that the network may have changed. */
object PairedNetwork {
    /** Learned routes are kept here between launches. They hold no secrets. */
    @Volatile var routesDirectory: String? = null
    private val listeners = CopyOnWriteArraySet<() -> Unit>()

    fun changed() = listeners.forEach { it() }

    internal fun observe(listener: () -> Unit): () -> Unit {
        listeners.add(listener)
        return { listeners.remove(listener) }
    }
}

/**
 * Owns the sockets of one paired connection. The Rust link decides which routes to open, when to
 * move and what to resend; this only performs that. Plaintext fallback does not exist here.
 */
class PairedSocket(
    private val link: String,
    private val http: OkHttpClient,
    private val receive: (String) -> Unit,
    private val failed: (Throwable) -> Unit,
    private val onRoute: ((PairedRouteStatus) -> Unit)? = null,
) : Closeable {
    private val lock = Any()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val ready = CompletableDeferred<Unit>()
    private val sockets = HashMap<Long, WebSocket>()
    private var handle: Long? = null
    private var wake: Job? = null
    private var stopObserving: (() -> Unit)? = null
    private var closed = false

    private fun request(op: String, vararg fields: Pair<String, JsonElement>): JsonObject =
        NativePairing.request(obj("op" to op.json(), "handle" to handle?.json(), *fields))

    private fun decode(value: JsonElement): ByteArray =
        Base64.getDecoder().decode(value.jsonPrimitive.content)

    private fun apply(result: JsonObject) {
        result.array("open").forEach {
            open(requireNotNull(it.long("socket")), requireNotNull(it.text("url")))
        }
        result.array("close").forEach { sockets.remove(it.jsonPrimitive.long)?.cancel() }
        result.array("write").forEach { write ->
            val socket = sockets[write.long("socket")]
            write.array("records").forEach { socket?.send(decode(it).toByteString()) }
        }
        result.array("deliver").forEach { receive(decode(it).toString(Charsets.UTF_8)) }
        result.field("status")?.let { status ->
            status.text("state")?.let { state ->
                onRoute?.invoke(
                    PairedRouteStatus(
                        state,
                        status.text("label"),
                        status.long("rttMs"),
                        status.flag("resumable"),
                    )
                )
            }
        }
        if (result.flag("connected")) ready.complete(Unit)
        if (result.text("ended") != null) {
            handle = null
            fail()
            return
        }
        wake?.cancel()
        val delayMs = result.long("waitMs") ?: 1000
        wake =
            scope.launch {
                delay(delayMs)
                event { apply(request("linkTick")) }
            }
    }

    private fun open(id: Long, url: String) {
        sockets[id] =
            http.newWebSocket(
                Request.Builder().url(url).build(),
                object : WebSocketListener() {
                    override fun onOpen(webSocket: WebSocket, response: Response) = event {
                        if (sockets[id] != null)
                            apply(request("linkSocketOpened", "socket" to id.json()))
                    }

                    override fun onMessage(webSocket: WebSocket, bytes: ByteString) = event {
                        require(bytes.size <= 65_535) { "Invalid paired record." }
                        if (sockets[id] != null)
                            apply(
                                request(
                                    "linkRecord",
                                    "socket" to id.json(),
                                    "record" to bytes.base64().json(),
                                )
                            )
                    }

                    override fun onMessage(webSocket: WebSocket, text: String) = lost(id)

                    override fun onFailure(
                        webSocket: WebSocket,
                        t: Throwable,
                        response: Response?,
                    ) = lost(id)

                    override fun onClosed(webSocket: WebSocket, code: Int, reason: String) =
                        lost(id)

                    override fun onClosing(webSocket: WebSocket, code: Int, reason: String) =
                        lost(id)
                },
            )
    }

    private fun lost(id: Long) = event {
        if (sockets.remove(id)?.also { it.cancel() } != null)
            apply(request("linkSocketClosed", "socket" to id.json()))
    }

    suspend fun connect() {
        try {
            synchronized(lock) {
                check(!closed && handle == null)
                val opened =
                    NativePairing.request(
                        obj(
                            "op" to "linkOpen".json(),
                            "link" to link.json(),
                            "store" to PairedNetwork.routesDirectory?.json(),
                        )
                    )
                handle = requireNotNull(opened.long("handle"))
                stopObserving =
                    PairedNetwork.observe { event { apply(request("linkNetworkChanged")) } }
                apply(opened)
            }
            withTimeout(30_000) { ready.await() }
        } catch (failure: Throwable) {
            close()
            throw failure
        }
    }

    fun send(text: String) = event {
        check(ready.isCompleted && !closed)
        val result =
            request(
                "linkSend",
                "payload" to
                    Base64.getEncoder().encodeToString(text.toByteArray(Charsets.UTF_8)).json(),
            )
        check(result.flag("accepted")) { "Paired connection closed." }
        apply(result)
    }

    private fun event(action: () -> Unit) {
        try {
            synchronized(lock) {
                if (closed) return
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
                stopObserving?.invoke()
                stopObserving = null
                scope.cancel()
                sockets.values.forEach { it.cancel() }
                sockets.clear()
                handle?.let { runCatching { request("linkClose") } }
                handle = null
            }
        }
}
