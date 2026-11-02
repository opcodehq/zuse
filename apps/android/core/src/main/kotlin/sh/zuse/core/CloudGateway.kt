package sh.zuse.core

import kotlinx.serialization.json.*
import okhttp3.HttpUrl.Companion.toHttpUrl

internal object CloudGateway {
    const val RUST = "zuse-workspace-rust-v1"
    const val RPC = "zuse-rpc-v1"
    const val LEGACY = "zuse-workspace-v3"
    const val EFFECT = "zuse-effect-rpc-v1"

    fun protocol(workspace: JsonElement): String =
        when (workspace.text("rpcProtocol")) {
            RPC -> RUST
            null,
            EFFECT -> LEGACY
            else -> throw ClientException("This cloud runtime requires a newer app.")
        }

    fun validate(ticket: JsonElement, workspace: String, protocol: String) {
        require(ticket.text("wsUrl")?.startsWith("wss://") == true)
        val url =
            requireNotNull(ticket.text("wsUrl"))
                .replaceFirst("wss://", "https://")
                .replaceFirst("ws://", "http://")
                .toHttpUrl()
        val expectedPath =
            url.newBuilder()
                .encodedPath("/")
                .addPathSegments("v1/cloud/workspaces")
                .addPathSegment(workspace)
                .addPathSegment("gateway")
                .build()
                .encodedPath
        require(ticket.text("workspaceId") == workspace && ticket.text("protocol") == protocol)
        require(ticket.text("rpcProtocol") == if (protocol == RUST) RPC else EFFECT)
        require(
            url.isHttps &&
                url.username.isEmpty() &&
                url.password.isEmpty() &&
                url.query == null &&
                url.fragment == null &&
                url.encodedPath == expectedPath
        )
        require(
            (ticket.long("generation") ?: 0) >= 1 &&
                (ticket.long("gatewayEpoch") ?: 0) >= 1 &&
                (ticket.long("expiresAt") ?: 0) > System.currentTimeMillis()
        )
        require(ticket.text("credential")?.matches(Regex("[A-Za-z0-9._~-]{1,16384}")) == true)
    }
}
