package sh.zuse.core

import java.io.Closeable
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.HttpUrl.Companion.toHttpUrl
import okio.ByteString

/** A client owns one socket generation. Requests and mutations are never replayed. */
class RpcClient(
    private val address: String,
    private val protocols: List<String> = emptyList(),
    private val token: String? = null,
    private val http: OkHttpClient = defaultHttp(),
    private val pairingLink: String? = null,
    private val accountGrant: (suspend () -> AccountGrant)? = null,
    private val accountEnvironmentId: String? = null,
    private val cloudExpiresAt: Long? = null,
    private val onRoute: ((PairedRouteStatus) -> Unit)? = null,
    val canonical: Boolean =
        accountGrant != null ||
            pairingLink != null ||
            CloudGateway.RUST in protocols ||
            java.net.URI(address).path == "/api/rpc",
) : Closeable {
    private val pending = ConcurrentHashMap<String, Channel<JsonElement>>()
    private val ready = CompletableDeferred<Unit>()
    private val connected = CompletableDeferred<Unit>()
    private val terminal = CompletableDeferred<Throwable>()
    @Volatile private var closed = false
    private val gateway = protocols.any { it == CloudGateway.LEGACY || it == CloudGateway.RUST }
    @Volatile private var available = !gateway
    private val lifetime = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var socket: WebSocket? = null
    private var paired: PairedSocket? = null
    private var account: AccountSocket? = null
    private val nextId = AtomicLong()

    suspend fun connect() {
        check(socket == null && paired == null && account == null && !closed)
        if (accountGrant != null) {
            try {
                account =
                    AccountSocket(
                        requireNotNull(accountEnvironmentId),
                        accountGrant,
                        http,
                        ::receive,
                        ::fail,
                    )
                account!!.connect()
                check(call("EngineInfo").text("deviceId") == accountEnvironmentId)
                return
            } catch (error: Throwable) {
                close()
                throw error
            }
        }
        if (pairingLink != null) {
            try {
                paired = PairedSocket(pairingLink, http, ::receive, ::fail, onRoute)
                paired!!.connect()
                call("EngineInfo")
                return
            } catch (error: Throwable) {
                close()
                throw error
            }
        }
        Endpoint.validate(address)
        val base = address.replaceFirst("wss://", "https://").replaceFirst("ws://", "http://")
        val url =
            base
                .toHttpUrl()
                .newBuilder()
                .apply { if (!canonical) setQueryParameter("wireVersion", "7") }
                .apply {
                    if (!canonical && !token.isNullOrEmpty()) setQueryParameter("token", token)
                }
                .build()
        val request =
            Request.Builder()
                .url(url)
                .apply {
                    if (canonical) {
                        header("Origin", "${url.scheme}://${url.toUri().rawAuthority}")
                        if (!token.isNullOrEmpty()) header("Authorization", "Bearer $token")
                    }
                    if (protocols.isNotEmpty())
                        header("Sec-WebSocket-Protocol", protocols.joinToString(", "))
                }
                .build()
        socket =
            http.newWebSocket(
                request,
                object : WebSocketListener() {
                    override fun onOpen(webSocket: WebSocket, response: Response) {
                        if (
                            CloudGateway.RUST in protocols &&
                                response.header("Sec-WebSocket-Protocol") != CloudGateway.RUST
                        ) {
                            fail(ClientException("Cloud gateway protocol was not confirmed."))
                            return
                        }
                        connected.complete(Unit)
                        if (available) ready.complete(Unit)
                    }

                    override fun onMessage(webSocket: WebSocket, text: String) = receive(text)

                    override fun onMessage(webSocket: WebSocket, bytes: ByteString) =
                        receive(bytes.utf8())

                    override fun onFailure(
                        webSocket: WebSocket,
                        t: Throwable,
                        response: Response?,
                    ) {
                        fail(ClientException("Connection lost. Reconnect to synchronize."))
                    }

                    override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                        webSocket.close(code, null)
                        fail(ClientException("The server closed the connection."))
                    }

                    override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                        fail(ClientException("The connection ended."))
                    }
                },
            )
        try {
            cloudExpiresAt?.let { expiry ->
                val remaining = expiry - System.currentTimeMillis()
                check(remaining > 0)
                lifetime.launch {
                    delay(remaining)
                    fail(ClientException("Cloud authorization expired. Reconnect to continue."))
                }
            }
            withTimeout(45_000) {
                connected.await()
                ready.await()
            }
            if (canonical) {
                call("EngineInfo")
                return
            }
            val welcome = call("connect.handshake", obj("protocolVersion" to 7.json()))
            require(welcome.long("protocolVersion") == 7L) {
                "Update the runtime and app to compatible versions."
            }
        } catch (error: Throwable) {
            close()
            throw error
        }
    }

    suspend fun call(method: String, payload: JsonObject = obj()): JsonElement =
        try {
            withTimeout(30_000) {
                var result: JsonElement = JsonNull
                stream(method, payload).collect { result = it }
                result
            }
        } catch (_: TimeoutCancellationException) {
            throw ClientException(
                "The request timed out. Its outcome may be unknown; refresh before retrying."
            )
        }

    fun stream(method: String, payload: JsonObject = obj()): Flow<JsonElement> = flow {
        check(!closed && available) { "The connection is unavailable." }
        val id =
            if (canonical) nextId.incrementAndGet().toString() else UUID.randomUUID().toString()
        val channel = Channel<JsonElement>(512)
        pending[id] = channel
        if (closed) {
            pending.remove(id)
            channel.close(ClientException("Connection closed."))
        }
        try {
            send(
                if (canonical)
                    obj("id" to id.toLong().json(), "method" to method.json(), "params" to payload)
                else
                    obj(
                        "_tag" to "Request".json(),
                        "id" to id.json(),
                        "tag" to method.json(),
                        "payload" to payload,
                        "headers" to JsonArray(emptyList()),
                    )
            )
            for (value in channel) emit(value)
        } finally {
            if (pending.remove(id) != null && !closed)
                runCatching {
                    send(
                        if (canonical) obj("id" to id.toLong().json(), "cancel" to true.json())
                        else obj("_tag" to "Interrupt".json(), "requestId" to id.json())
                    )
                }
            channel.cancel()
        }
    }

    suspend fun awaitDisconnect(): Throwable = terminal.await()

    private fun receive(text: String) {
        try {
            text
                .lineSequence()
                .filter { it.isNotBlank() }
                .forEach { line ->
                    val parsed = WireJson.parseToJsonElement(line)
                    (if (parsed is JsonArray) parsed.toList() else listOf(parsed)).forEach(::frame)
                }
        } catch (error: Throwable) {
            // A reply that cannot be read may belong to a request the host already ran,
            // so pending calls must see a connection failure, never an argument error.
            fail(
                error as? ClientException
                    ?: ClientException(
                            "The host sent a reply that could not be read. Reconnect to synchronize."
                        )
                        .apply { initCause(error) }
            )
        }
    }

    private fun frame(frame: JsonElement) {
        if (frame.text("_zuseGateway") == "availability") {
            if (frame.flag("reset") || (available && gateway))
                throw ClientException("The workspace runtime changed. Reconnect to continue.")
            when (frame.text("state")) {
                "available" -> {
                    available = true
                    ready.complete(Unit)
                }
                "pending" -> Unit
                else -> throw ClientException("Invalid cloud runtime state.")
            }
            return
        }
        if (gateway && !available) throw ClientException("Cloud runtime was not ready.")
        if (canonical) {
            val id = frame.text("id") ?: return
            val channel = pending[id] ?: return
            val fields = frame as? JsonObject ?: return
            when {
                "err" in fields -> {
                    pending.remove(id)
                    channel.close(RpcRequestFailure(null, frame.text("err") ?: "Request failed"))
                }
                "ok" in fields -> {
                    offer(channel, fields.getValue("ok"))
                    pending.remove(id)
                    channel.close()
                }
                "item" in fields -> offer(channel, fields.getValue("item"))
                frame.flag("done") -> {
                    pending.remove(id)
                    channel.close()
                }
            }
            return
        }
        when (frame.text("_tag")) {
            "Ping" -> {
                send(obj("_tag" to "Pong".json()))
                return
            }
            "Pong" -> return
            "Defect" -> throw ClientException("The runtime rejected the connection.")
        }
        val id = frame.text("requestId") ?: return
        val channel = pending[id] ?: return
        when (frame.text("_tag")) {
            "Chunk" -> {
                frame.array("values").forEach { if (!offer(channel, it)) return }
                send(obj("_tag" to "Ack".json(), "requestId" to id.json()))
            }
            "Exit" -> {
                val exit = frame.field("exit")
                if (exit.text("_tag") == "Success") {
                    val value = exit.field("value")
                    if (value != JsonNull && !offer(channel, value)) return
                    pending.remove(id)
                    channel.close()
                } else {
                    pending.remove(id)
                    val error = exit.array("cause").firstOrNull()?.field("error")
                    channel.close(
                        RpcRequestFailure(
                            error?.text("_tag"),
                            error?.text("message")
                                ?: error?.text("_tag")
                                ?: "The runtime rejected the request.",
                        )
                    )
                }
            }
        }
    }

    private fun offer(channel: Channel<JsonElement>, value: JsonElement): Boolean {
        val result = channel.trySend(value)
        // Cancellation may race a frame already read from the socket. It does not invalidate other
        // requests.
        if (result.isClosed) return false
        if (result.isFailure)
            throw ClientException("Response buffer exceeded. Reconnect to recover missing updates.")
        return true
    }

    private fun send(value: JsonElement) {
        account?.let {
            it.send(value.toString())
            return
        }
        paired?.let {
            it.send(value.toString())
            check(!closed) { "Paired connection closed." }
            return
        }
        check(socket?.send(value.toString()) == true) {
            "The request could not be sent. Reconnect before retrying."
        }
    }

    @Synchronized
    private fun fail(error: Throwable) {
        if (closed) return
        closed = true
        connected.completeExceptionally(error)
        ready.completeExceptionally(error)
        terminal.complete(error)
        pending.values.forEach { it.close(error) }
        pending.clear()
        socket?.cancel()
        paired?.close()
        account?.close()
        lifetime.cancel()
    }

    override fun close() = fail(ClientException("Connection closed."))

    companion object {
        fun defaultHttp(): OkHttpClient =
            OkHttpClient.Builder()
                .connectTimeout(15, TimeUnit.SECONDS)
                .readTimeout(0, TimeUnit.SECONDS)
                .pingInterval(20, TimeUnit.SECONDS)
                .followRedirects(false)
                .followSslRedirects(false)
                .build()
    }
}
