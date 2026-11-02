package sh.zuse.core

import java.util.UUID
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.selects.*
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.HttpUrl.Companion.toHttpUrl
import okio.ByteString

/** Transient account grant. Never persist it with remembered computer metadata. */
data class AccountGrant(
    val environmentId: String,
    val origin: String,
    val token: String,
    val expiresAt: Long,
) {
    override fun toString() = "AccountGrant(environmentId=$environmentId, expiresAt=$expiresAt)"

    fun socketUrl(): String {
        val url = origin.toHttpUrl()
        require(
            url.username.isEmpty() &&
                url.password.isEmpty() &&
                url.encodedPath == "/" &&
                url.query == null &&
                url.fragment == null
        )
        require(url.isHttps || url.host in listOf("localhost", "127.0.0.1", "::1"))
        require(
            environmentId.isNotBlank() &&
                token.isNotBlank() &&
                token.length <= 16384 &&
                expiresAt > System.currentTimeMillis()
        )
        return url.newBuilder().addPathSegments("api/account-rpc").build().toString()
    }
}

/** Authenticates before exposing RPC, renews grants and expires even while the socket is idle. */
internal class AccountSocket(
    private val expectedId: String,
    private val grant: suspend () -> AccountGrant,
    private val http: OkHttpClient,
    private val receive: (String) -> Unit,
    private val failed: (Throwable) -> Unit,
) : java.io.Closeable {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val frames = Channel<String>(128)
    private val opened = CompletableDeferred<Unit>()
    private val ready = CompletableDeferred<Unit>()
    private var socket: WebSocket? = null
    @Volatile private var active = false

    suspend fun connect() {
        scope.launch {
            try {
                run()
            } catch (error: Throwable) {
                ready.completeExceptionally(error)
                failed(ClientException("Account connection ended. Select the computer again."))
                close()
            }
        }
        try {
            withTimeout(30_000) { ready.await() }
        } catch (error: Throwable) {
            close()
            throw error
        }
    }

    private fun sendControl(value: JsonObject) {
        check(socket?.send(value.toString()) == true)
    }

    fun send(text: String) {
        check(active && socket?.send(text) == true)
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private suspend fun run() = coroutineScope {
        val first = withTimeout(10_000) { grant() }
        check(first.environmentId == expectedId)
        val address = first.socketUrl()
        socket =
            http.newWebSocket(
                Request.Builder().url(address).header("Sec-WebSocket-Protocol", PROTOCOL).build(),
                object : WebSocketListener() {
                    override fun onOpen(webSocket: WebSocket, response: Response) {
                        opened.complete(Unit)
                    }

                    override fun onMessage(webSocket: WebSocket, text: String) {
                        if (frames.trySend(text).isFailure)
                            frames.close(ClientException("Account response buffer exceeded."))
                    }

                    override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                        frames.close(ClientException("Unexpected account frame."))
                    }

                    override fun onFailure(
                        webSocket: WebSocket,
                        t: Throwable,
                        response: Response?,
                    ) {
                        opened.completeExceptionally(ClientException("Account connection failed."))
                        frames.close(ClientException("Account connection failed."))
                    }

                    override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                        frames.close(ClientException("Account connection closed."))
                        webSocket.close(code, null)
                    }
                },
            )
        withTimeout(10_000) { opened.await() }
        fun authenticate(value: AccountGrant) =
            sendControl(obj("protocol" to PROTOCOL.json(), "connectToken" to value.token.json()))
        fun acknowledge(text: String, value: AccountGrant, renewed: Boolean): Long {
            val frame = WireJson.parseToJsonElement(text)
            val until = requireNotNull(frame.long("expiresAt"))
            check(
                frame.flag("authenticated") &&
                    frame.text("protocol") == PROTOCOL &&
                    frame.text("deviceId") == expectedId &&
                    frame.flag("renewed") == renewed &&
                    until > System.currentTimeMillis() &&
                    until <= value.expiresAt + 1000
            )
            return until
        }
        authenticate(first)
        var until = acknowledge(withTimeout(10_000) { frames.receive() }, first, false)
        var monotonicExpiry = System.nanoTime() + (until - System.currentTimeMillis()) * 1_000_000
        fun renewalTime(expiry: Long): Long {
            val remaining = expiry - System.currentTimeMillis()
            return System.currentTimeMillis() +
                (remaining - (remaining / 3).coerceIn(1, 30_000)).coerceAtLeast(1)
        }
        var renewAt = renewalTime(until)
        var renewal: Deferred<AccountGrant>? = null
        var awaiting: AccountGrant? = null
        var ackDeadline = Long.MAX_VALUE
        var pingAt = System.nanoTime() + 20_000_000_000
        var pong: String? = null
        var pongDeadline = Long.MAX_VALUE
        active = true
        ready.complete(Unit)
        while (isActive) {
            val now = System.nanoTime()
            check(
                System.currentTimeMillis() < until &&
                    now < monotonicExpiry &&
                    now < ackDeadline &&
                    now < pongDeadline
            )
            if (renewal == null && awaiting == null && System.currentTimeMillis() >= renewAt)
                renewal = async { withTimeout(10_000) { grant() } }
            if (pong == null && now >= pingAt) {
                pong = UUID.randomUUID().toString()
                pongDeadline = now + 10_000_000_000
                sendControl(obj("protocol" to PROTOCOL.json(), "ping" to pong.json()))
            }
            select<Unit> {
                frames.onReceive { text ->
                    val value = WireJson.parseToJsonElement(text)
                    if (value.field("protocol") != JsonNull) {
                        check(value.text("protocol") == PROTOCOL)
                        if (value.text("pong") != null) {
                            check(pong != null && value.text("pong") == pong)
                            pong = null
                            pongDeadline = Long.MAX_VALUE
                            pingAt = System.nanoTime() + 20_000_000_000
                        } else {
                            val pending = requireNotNull(awaiting)
                            until = acknowledge(text, pending, true)
                            monotonicExpiry =
                                System.nanoTime() + (until - System.currentTimeMillis()) * 1_000_000
                            awaiting = null
                            ackDeadline = Long.MAX_VALUE
                            renewAt = renewalTime(until)
                        }
                    } else {
                        check(awaiting == null || System.nanoTime() < ackDeadline)
                        receive(text)
                    }
                }
                renewal?.let { task ->
                    task.onAwait { next ->
                        renewal = null
                        check(next.environmentId == expectedId && next.socketUrl() == address)
                        if (next.expiresAt > until + 1000) {
                            awaiting = next
                            ackDeadline = System.nanoTime() + 10_000_000_000
                            authenticate(next)
                        } else renewAt = System.currentTimeMillis() + 1000
                    }
                }
                onTimeout(250) {}
            }
        }
    }

    override fun close() {
        active = false
        scope.cancel()
        socket?.cancel()
        frames.cancel()
    }

    companion object {
        const val PROTOCOL = "zuse-rpc-json-v1"
    }
}
