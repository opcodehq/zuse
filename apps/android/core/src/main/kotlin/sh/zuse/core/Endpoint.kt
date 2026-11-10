package sh.zuse.core

import java.net.URI
import java.net.URLDecoder

/**
 * Keep cleartext scoped to LAN, loopback, and Tailscale addresses; never resolve arbitrary DNS
 * here.
 */
object Endpoint {
    fun isPrivateHost(value: String): Boolean {
        val host = value.lowercase().removePrefix("[").removeSuffix("]").trimEnd('.')
        if (
            host == "localhost" ||
                host.endsWith(".localhost") ||
                host.endsWith(".local") ||
                host.endsWith(".ts.net") ||
                host == "::1"
        )
            return true
        val octets = host.split('.').map { it.toIntOrNull() }
        if (octets.size == 4 && octets.all { it != null && it in 0..255 }) {
            val a = octets.map { it!! }
            return a[0] == 127 ||
                a[0] == 10 ||
                (a[0] == 172 && a[1] in 16..31) ||
                (a[0] == 192 && a[1] == 168) ||
                (a[0] == 169 && a[1] == 254) ||
                (a[0] == 100 && a[1] in 64..127)
        }
        return host.contains(':') &&
            (host.startsWith("fc") || host.startsWith("fd") || host.startsWith("fe80:"))
    }

    fun validate(value: String, websocket: Boolean = true): URI {
        val uri =
            try {
                URI(value.trim())
            } catch (_: Exception) {
                throw ClientException("Enter a valid server address.")
            }
        val secure = if (websocket) "wss" else "https"
        val local = if (websocket) "ws" else "http"
        require(
            uri.host != null &&
                uri.rawUserInfo == null &&
                uri.fragment == null &&
                (uri.port == -1 || uri.port in 1..65535) &&
                (uri.scheme == secure || (uri.scheme == local && isPrivateHost(uri.host)))
        ) {
            "Use TLS for public servers, or a private LAN/Tailscale address."
        }
        return uri
    }

    fun query(value: String?): Map<String, String> =
        value
            .orEmpty()
            .split('&')
            .filter { it.isNotEmpty() }
            .associate {
                val pair = it.split('=', limit = 2)
                URLDecoder.decode(pair[0], "UTF-8") to
                    URLDecoder.decode(pair.getOrElse(1) { "" }, "UTF-8")
            }

    /** Route versioned native invitations separately from legacy one-time pairing codes. */
    fun isNativeInvitation(value: String): Boolean {
        val uri = runCatching { URI(value.trim()) }.getOrNull() ?: return false
        val fragment = query(uri.rawFragment)["pair"] ?: return false
        val decoded =
            runCatching {
                    java.util.Base64.getUrlDecoder().decode(fragment).toString(Charsets.UTF_8)
                }
                .getOrNull() ?: return false
        val fields =
            runCatching { WireJson.parseToJsonElement(decoded) }.getOrNull() ?: return false
        return fields.field("v") != kotlinx.serialization.json.JsonNull ||
            fields.field("id") != kotlinx.serialization.json.JsonNull
    }

    data class Pairing(val http: String, val websocket: String, val code: String)

    fun pairing(value: String): Pairing {
        val outer = URI(value.trim())
        require(outer.rawUserInfo == null) { "Pairing links cannot contain embedded credentials." }
        val fragment = query(outer.rawFragment)
        val ws: URI
        val code: String
        if (outer.scheme in listOf("http", "https")) {
            code = fragment["pair"].orEmpty()
            ws =
                URI(
                    if (outer.scheme == "https") "wss" else "ws",
                    null,
                    outer.host,
                    outer.port,
                    "/rpc",
                    null,
                    null,
                )
        } else {
            require(outer.scheme == "zuse" || outer.scheme == "memoize") {
                "This is not a Zuse pairing link."
            }
            val endpoint =
                URI(
                    query(outer.rawQuery)["pairingUrl"]
                        ?: throw ClientException("Missing pairing address.")
                )
            code = fragment["token"].orEmpty()
            ws =
                if (endpoint.path.isNullOrEmpty() || endpoint.path == "/")
                    URI(
                        endpoint.scheme,
                        null,
                        endpoint.host,
                        endpoint.port,
                        "/rpc",
                        endpoint.rawQuery,
                        endpoint.rawFragment,
                    )
                else endpoint
        }
        require(code.isNotBlank() && ws.query == null) { "The pairing link is incomplete." }
        validate(ws.toString())
        return Pairing(
            URI(if (ws.scheme == "wss") "https" else "http", null, ws.host, ws.port, "", null, null)
                .toString(),
            ws.toString(),
            code,
        )
    }
}
