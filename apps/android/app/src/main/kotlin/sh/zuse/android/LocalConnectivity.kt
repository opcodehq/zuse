package sh.zuse.android

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import java.net.URI
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.X509Certificate
import javax.net.ssl.*
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.callbackFlow
import okhttp3.OkHttpClient
import sh.zuse.core.RpcClient

/** Pins are SHA-256 of the certificate DER, matching zuse serve's discovery contract. */
fun pinnedHttp(pin: String?): OkHttpClient {
    if (pin == null) return RpcClient.defaultHttp()
    require(Regex("[A-Za-z0-9_-]{43}").matches(pin)) { "Invalid computer certificate pin." }
    fun matches(cert: X509Certificate) =
        MessageDigest.isEqual(
            MessageDigest.getInstance("SHA-256").digest(cert.encoded).base64Url().toByteArray(),
            pin.toByteArray(),
        )
    val trust =
        object : X509TrustManager {
            override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()

            override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
                throw java.security.cert.CertificateException("Client certificates unsupported")
            }

            override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
                if (chain.isEmpty() || !matches(chain[0]))
                    throw java.security.cert.CertificateException(
                        "The computer certificate changed. Pair again."
                    )
            }
        }
    val tls = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), SecureRandom()) }
    return RpcClient.defaultHttp()
        .newBuilder()
        .sslSocketFactory(tls.socketFactory, trust)
        .hostnameVerifier { _, session ->
            (session.peerCertificates.firstOrNull() as? X509Certificate)?.let(::matches) == true
        }
        .build()
}

data class NearbyComputer(val name: String, val address: String, val pin: String)

class NearbyBrowser(context: Context) {
    private val manager = context.getSystemService(NsdManager::class.java)
    val computers = callbackFlow {
        val present = java.util.concurrent.ConcurrentHashMap.newKeySet<String>()
        val found = java.util.concurrent.ConcurrentHashMap<String, NearbyComputer>()
        val queue = java.util.concurrent.ConcurrentLinkedQueue<NsdServiceInfo>()
        val resolving = java.util.concurrent.atomic.AtomicBoolean(false)
        val active = java.util.concurrent.atomic.AtomicBoolean(true)
        fun publish() {
            if (active.get()) trySend(found.values.sortedBy { it.name })
        }
        fun resolveNext() {
            if (!active.get() || !resolving.compareAndSet(false, true)) return
            val service = queue.poll()
            if (service == null) {
                resolving.set(false)
                return
            }
            @Suppress("DEPRECATION")
            manager.resolveService(
                service,
                object : NsdManager.ResolveListener {
                    override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                        resolving.set(false)
                        resolveNext()
                    }

                    override fun onServiceResolved(info: NsdServiceInfo) {
                        val pin = info.attributes["tls"]?.toString(Charsets.UTF_8)
                        val host = info.host?.hostAddress
                        if (
                            active.get() &&
                                present.contains(info.serviceName) &&
                                pin != null &&
                                Regex("[A-Za-z0-9_-]{43}").matches(pin) &&
                                host != null
                        ) {
                            val address =
                                URI("https", null, host, info.port, "", null, null).toString()
                            found[info.serviceName] = NearbyComputer(info.serviceName, address, pin)
                            publish()
                        }
                        resolving.set(false)
                        resolveNext()
                    }
                },
            )
        }
        val listener =
            object : NsdManager.DiscoveryListener {
                override fun onDiscoveryStarted(serviceType: String) {}

                override fun onDiscoveryStopped(serviceType: String) {}

                override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                    close(
                        IllegalStateException(
                            "Nearby discovery unavailable. Paste a pairing link instead."
                        )
                    )
                }

                override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}

                override fun onServiceFound(serviceInfo: NsdServiceInfo) {
                    present.add(serviceInfo.serviceName)
                    queue.add(serviceInfo)
                    resolveNext()
                }

                override fun onServiceLost(serviceInfo: NsdServiceInfo) {
                    present.remove(serviceInfo.serviceName)
                    found.remove(serviceInfo.serviceName)
                    queue.removeIf { it.serviceName == serviceInfo.serviceName }
                    publish()
                }
            }
        trySend(emptyList())
        manager.discoverServices("_zuse._tcp.", NsdManager.PROTOCOL_DNS_SD, listener)
        awaitClose {
            active.set(false)
            queue.clear()
            runCatching { manager.stopServiceDiscovery(listener) }
        }
    }
}
