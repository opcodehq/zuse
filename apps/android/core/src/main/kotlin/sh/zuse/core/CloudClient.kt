package sh.zuse.core

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody

class CloudClient(
    val baseUrl: String,
    private val token: suspend () -> String,
    private val http: OkHttpClient =
        RpcClient.defaultHttp()
            .newBuilder()
            .callTimeout(30, java.util.concurrent.TimeUnit.SECONDS)
            .readTimeout(30, java.util.concurrent.TimeUnit.SECONDS)
            .build(),
    private val allowLoopbackHttp: Boolean = false,
) {
    suspend fun request(
        path: String,
        method: String = "GET",
        body: JsonElement? = null,
        proof: ((String, String, String?) -> String)? = null,
        deviceToken: String? = null,
    ): JsonElement {
        val base = baseUrl.toHttpUrl()
        val localHttp = allowLoopbackHttp && base.scheme == "http" &&
            (base.host == "127.0.0.1" || base.host == "::1")
        require((base.isHttps || localHttp) && base.username.isEmpty() && base.password.isEmpty() &&
            base.encodedPath == "/" && base.query == null && base.fragment == null) {
            "Cloud requires HTTPS."
        }
        val url = base.resolve(path) ?: throw ClientException("Invalid cloud address.")
        require(url.host == base.host && url.port == base.port && url.scheme == base.scheme)
        val credential = deviceToken ?: token()
        return withContext(Dispatchers.IO) {
            val builder =
                Request.Builder()
                    .url(url)
                    .header(
                        "Authorization",
                        "${if (deviceToken == null) "Bearer" else "DPoP"} $credential",
                    )
            if (proof != null) builder.header("DPoP", proof(method, url.toString(), deviceToken))
            builder.method(
                method,
                if (method == "GET" || method == "HEAD") null
                else (body ?: obj()).toString().toRequestBody("application/json".toMediaType()),
            )
            http.newCall(builder.build()).execute().use { response ->
                if (!response.isSuccessful)
                    throw HttpRequestFailure(
                        response.code,
                        if (response.code == 401) "Your account session expired. Sign in again."
                        else "Cloud request failed (${response.code}).",
                    )
                val text = response.body?.string().orEmpty()
                if (text.isBlank()) JsonNull else WireJson.parseToJsonElement(text)
            }
        }
    }

    suspend fun chats(): List<CloudChat> =
        request("/v1/cloud/workspaces")
            .array("workspaces")
            .filter {
                it.text("state") !in listOf("deleted", "deleting", "archived", "archiving") &&
                    it.text("desiredState") != "archived"
            }
            .map { row ->
                CloudChat(
                    workspaceId = requireNotNull(row.text("workspaceId")),
                    projectId = requireNotNull(row.text("projectId")),
                    chatId = requireNotNull(row.text("chatId")),
                    initialSessionId = requireNotNull(row.text("initialSessionId")),
                    title = row.text("title") ?: "Cloud chat",
                    repositoryDisplayName =
                        row.text("repositoryDisplayName") ?: row.text("projectId").orEmpty(),
                    state = row.text("state") ?: "unknown",
                    runtimeState = row.text("runtimeState") ?: "unknown",
                )
            }

    suspend fun environments(): List<RemoteEnvironment> =
        request("/v1/environments").field("environments").decode()

    suspend fun connect(workspaceId: String): RpcClient {
        val id =
            HttpUrl.Builder()
                .scheme("https")
                .host("local")
                .addPathSegment(workspaceId)
                .build()
                .encodedPath
                .removePrefix("/")
        val workspace = request("/v1/cloud/workspaces/$id")
        val protocol = CloudGateway.protocol(workspace)
        if (workspace.text("state") == "paused" && workspace.text("desiredState") != "ready") {
            val resumed =
                request(
                    "/v1/cloud/workspaces/$id/resume",
                    "POST",
                    obj(
                        "workspaceId" to workspaceId.json(),
                        "commandId" to java.util.UUID.randomUUID().toString().json(),
                    ),
                )
            require(resumed.text("workspaceId") == workspaceId) {
                "The cloud action identified another workspace."
            }
        }
        val ticket =
            request(
                "/v1/cloud/workspaces/$id/gateway/ticket",
                "POST",
                obj(
                    "protocol" to protocol.json(),
                    "rpcProtocol" to
                        (if (protocol == CloudGateway.RUST) CloudGateway.RPC
                            else CloudGateway.EFFECT)
                            .json(),
                ),
            )
        CloudGateway.validate(ticket, workspaceId, protocol)
        val address =
            ticket.text("wsUrl") ?: throw ClientException("Missing cloud gateway address.")
        require(address.startsWith("wss://"))
        val protocols =
            listOf(
                ticket.text("protocol") ?: throw ClientException("Missing gateway protocol."),
                ticket.text("credential") ?: throw ClientException("Missing gateway credential."),
            )
        val rpc =
            RpcClient(address, protocols, http = http, cloudExpiresAt = ticket.long("expiresAt"))
        try {
            rpc.connect()
            return rpc
        } catch (error: Throwable) {
            rpc.close()
            throw error
        }
    }

    suspend fun connectEnvironment(
        id: String,
        proof: (String, String, String?) -> String,
        canonicalAccount: Boolean = false,
    ): RpcClient {
        if (canonicalAccount) {
            val encoded =
                HttpUrl.Builder()
                    .scheme("https")
                    .host("local")
                    .addPathSegment(id)
                    .build()
                    .encodedPath
                    .removePrefix("/")
            val rpc =
                RpcClient(
                    "wss://account.invalid/api/account-rpc",
                    http = http,
                    accountEnvironmentId = id,
                    accountGrant = {
                        val device = request("/v1/client/dpop-token", "POST", proof = proof)
                        val access =
                            device.text("accessToken")
                                ?: throw ClientException("Missing device grant.")
                        val registrationPath = "/v1/mobile/devices"
                        val registrationProof =
                            proof(
                                "POST",
                                baseUrl.toHttpUrl().resolve(registrationPath).toString(),
                                access,
                            )
                        val header =
                            WireJson.parseToJsonElement(
                                String(
                                    java.util.Base64.getUrlDecoder()
                                        .decode(registrationProof.substringBefore('.'))
                                )
                            )
                        val jwk = header.field("jwk")
                        val keyId =
                            java.security.MessageDigest.getInstance("SHA-256")
                                .digest(jwk.toString().toByteArray())
                                .joinToString("") { "%02x".format(it) }
                        request(
                            registrationPath,
                            "POST",
                            obj(
                                "deviceId" to "android-$keyId".json(),
                                "platform" to "android".json(),
                                "dpopJwk" to jwk,
                            ),
                            proof = proof,
                            deviceToken = access,
                        )
                        val connection =
                            request(
                                "/v1/environments/$encoded/connect",
                                "POST",
                                obj(
                                    "requireManaged" to true.json(),
                                    "wireProtocolVersion" to 1.json(),
                                ),
                                proof = proof,
                                deviceToken = access,
                            )
                        val endpoint =
                            connection.field("endpoint").text("wsBaseUrl")
                                ?: throw ClientException("Missing computer address.")
                        require(endpoint.startsWith("wss://"))
                        AccountGrant(
                            id,
                            endpoint.replaceFirst("wss://", "https://"),
                            connection.text("connectToken")
                                ?: throw ClientException("Missing connection grant."),
                            connection.long("expiresAt")
                                ?: throw ClientException("Missing grant expiry."),
                        )
                    },
                )
            try {
                rpc.connect()
                return rpc
            } catch (error: Throwable) {
                rpc.close()
                throw error
            }
        }
        val grant = request("/v1/client/dpop-token", "POST", proof = proof)
        val encoded =
            HttpUrl.Builder()
                .scheme("https")
                .host("local")
                .addPathSegment(id)
                .build()
                .encodedPath
                .removePrefix("/")
        val connection =
            request(
                "/v1/environments/$encoded/connect",
                "POST",
                proof = proof,
                deviceToken =
                    grant.text("accessToken") ?: throw ClientException("Missing device grant."),
            )
        val endpoint =
            connection.field("endpoint").text("wsBaseUrl")
                ?: throw ClientException("Missing computer address.")
        require(endpoint.startsWith("wss://"))
        val rpc =
            RpcClient(
                endpoint,
                token =
                    connection.text("connectToken")
                        ?: throw ClientException("Missing connection grant."),
            )
        try {
            rpc.connect()
            require(rpc.call("connect.describe").text("environmentId") == id) {
                "The computer identity changed."
            }
            return rpc
        } catch (error: Throwable) {
            rpc.close()
            throw error
        }
    }
}
