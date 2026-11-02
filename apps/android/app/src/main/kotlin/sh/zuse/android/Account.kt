package sh.zuse.android

import android.net.Uri
import android.util.Base64
import java.security.MessageDigest
import java.security.SecureRandom
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import sh.zuse.core.*

class Account(private val vault: Vault) {
    private val mutex = Mutex()
    private val http =
        RpcClient.defaultHttp()
            .newBuilder()
            .callTimeout(30, java.util.concurrent.TimeUnit.SECONDS)
            .readTimeout(30, java.util.concurrent.TimeUnit.SECONDS)
            .build()
    private var generation = 0
    var signedIn = false
        private set

    var identity = "signed-out"
        private set

    suspend fun hydrate() {
        val credentials = vault.read("account")
        signedIn = credentials != null
        if (credentials != null) identity = identityOf(WireJson.parseToJsonElement(credentials))
    }

    private fun random(): String =
        ByteArray(32).apply { SecureRandom().nextBytes(this) }.base64Url()

    suspend fun authorize(): Uri {
        val verifier = random()
        val state = random()
        vault.write(
            "oauth",
            obj(
                    "verifier" to verifier.json(),
                    "state" to state.json(),
                    "time" to System.currentTimeMillis().json(),
                )
                .toString(),
        )
        return Uri.parse("https://api.workos.com/user_management/authorize")
            .buildUpon()
            .apply {
                mapOf(
                        "client_id" to BuildConfig.WORKOS_CLIENT_ID,
                        "redirect_uri" to "zuse://auth",
                        "response_type" to "code",
                        "provider" to "authkit",
                        "scope" to "openid profile email offline_access",
                        "code_challenge" to
                            MessageDigest.getInstance("SHA-256")
                                .digest(verifier.toByteArray())
                                .base64Url(),
                        "code_challenge_method" to "S256",
                        "state" to state,
                    )
                    .forEach { (key, value) -> appendQueryParameter(key, value) }
            }
            .build()
    }

    suspend fun callback(uri: Uri) =
        mutex.withLock {
            require(uri.scheme == "zuse" && uri.host == "auth")
            val pending =
                vault.read("oauth")?.let(WireJson::parseToJsonElement)
                    ?: throw ClientException("Sign-in expired. Try again.")
            require(
                uri.getQueryParameter("state") == pending.text("state") &&
                    System.currentTimeMillis() - (pending.long("time") ?: 0) < 600_000
            ) {
                "The sign-in response could not be verified."
            }
            vault.write("oauth", null)
            val epoch = generation
            val credentials =
                authenticate(
                    obj(
                        "grant_type" to "authorization_code".json(),
                        "code" to
                            (uri.getQueryParameter("code")
                                    ?: throw ClientException("Sign-in cancelled."))
                                .json(),
                        "code_verifier" to pending.field("verifier"),
                    )
                )
            check(epoch == generation)
            vault.write("account", credentials.toString())
            signedIn = true
            identity = identityOf(credentials)
        }

    suspend fun token(): String =
        mutex.withLock {
            val stored =
                vault.read("account")?.let(WireJson::parseToJsonElement)
                    ?: throw ClientException("Sign in to Zuse Cloud.")
            val access = stored.text("access_token") ?: throw ClientException("Sign in again.")
            val claims = claims(access)
            if ((claims.long("exp") ?: 0) * 1000 > System.currentTimeMillis() + 60_000)
                return@withLock access
            val epoch = generation
            val updated =
                authenticate(
                    obj(
                        "grant_type" to "refresh_token".json(),
                        "refresh_token" to stored.field("refresh_token"),
                    )
                )
            check(epoch == generation)
            vault.write("account", updated.toString())
            identity = identityOf(updated)
            updated.text("access_token") ?: throw ClientException("Invalid sign-in response.")
        }

    suspend fun signOut() =
        mutex.withLock {
            generation++
            signedIn = false
            identity = "signed-out"
            vault.write("account", null)
            vault.write("oauth", null)
        }

    private suspend fun authenticate(body: JsonObject): JsonElement =
        withContext(Dispatchers.IO) {
            val payload = JsonObject(body + ("client_id" to BuildConfig.WORKOS_CLIENT_ID.json()))
            val request =
                Request.Builder()
                    .url("https://api.workos.com/user_management/authenticate")
                    .post(payload.toString().toRequestBody("application/json".toMediaType()))
                    .build()
            http.newCall(request).execute().use { response ->
                if (!response.isSuccessful)
                    throw ClientException("Sign-in expired or was rejected. Please sign in again.")
                val result = WireJson.parseToJsonElement(response.body?.string().orEmpty())
                require(
                    !result.text("access_token").isNullOrBlank() &&
                        !result.text("refresh_token").isNullOrBlank()
                )
                result
            }
        }

    private fun claims(token: String): JsonElement =
        runCatching {
                WireJson.parseToJsonElement(
                    Base64.decode(token.split('.')[1], Base64.URL_SAFE or Base64.NO_WRAP)
                        .toString(Charsets.UTF_8)
                )
            }
            .getOrDefault(JsonNull)

    private fun identityOf(credentials: JsonElement): String {
        val claims = claims(credentials.text("access_token").orEmpty())
        return "${BuildConfig.API_URL}|${claims.text("sub") ?: throw ClientException("Missing account identity.")}|${claims.text("org_id") ?: "personal"}"
    }
}

fun ByteArray.base64Url(): String =
    Base64.encodeToString(this, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
