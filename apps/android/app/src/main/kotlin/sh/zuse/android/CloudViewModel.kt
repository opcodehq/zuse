package sh.zuse.android

import android.app.Application
import androidx.compose.runtime.*
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import okhttp3.HttpUrl
import sh.zuse.core.*

class CloudViewModel(application: Application) : AndroidViewModel(application) {
    private val app
        get() = getApplication<ZuseApplication>()

    private val cloud
        get() = CloudClient(BuildConfig.API_URL, app.account::token, allowLoopbackHttp = BuildConfig.DEBUG && BuildConfig.DEPLOYMENT_PROFILE == "staging")

    var projects by mutableStateOf<List<JsonElement>>(emptyList())
        private set

    var providers by mutableStateOf<List<JsonElement>>(emptyList())
        private set

    var agents by mutableStateOf<List<ModelChoice>>(emptyList())
        private set

    var auth by mutableStateOf<JsonElement>(JsonNull)
        private set

    var operation by mutableStateOf<JsonElement>(JsonNull)
        private set

    var busy by mutableStateOf(false)
        private set

    var failure by mutableStateOf<String?>(null)
        private set

    var pendingLaunch by mutableStateOf<JsonElement>(JsonNull)
        private set

    var pendingRepository by mutableStateOf<JsonElement>(JsonNull)
        private set

    private var polling: Job? = null
    private var activeIntentKey: String? = null

    private fun encoded(value: String) =
        HttpUrl.Builder()
            .scheme("https")
            .host("local")
            .addPathSegment(value)
            .build()
            .encodedPath
            .removePrefix("/")

    private fun action(block: suspend () -> Unit) =
        viewModelScope.launch {
            if (busy) return@launch
            busy = true
            failure = null
            activeIntentKey = null
            try {
                block()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Throwable) {
                if (
                    error is HttpRequestFailure &&
                        error.status in 400..499 &&
                        error.status !in listOf(408, 409, 429)
                ) {
                    activeIntentKey?.let { app.vault.write(it, null) }
                    if (activeIntentKey?.endsWith(":launch") == true) pendingLaunch = JsonNull
                }
                failure = error.message ?: "Cloud operation failed."
            } finally {
                busy = false
            }
        }

    fun load() = action { loadData() }

    private suspend fun loadData() = coroutineScope {
        val identity = app.account.identity
        val projectRequest = async { cloud.request("/v1/cloud/projects") }
        val providerRequest = async { cloud.request("/v1/cloud/providers") }
        val authRequest = async { cloud.request("/v1/cloud/auth") }
        val p = projectRequest.await()
        val c = providerRequest.await()
        val a = authRequest.await()
        check(identity == app.account.identity)
        projects = p.array("projects")
        providers = c.array("providers")
        auth = a
        val catalog =
            withContext(Dispatchers.IO) {
                app.assets.open("ModelCatalog.json").bufferedReader().use {
                    WireJson.parseToJsonElement(it.readText())
                }
            }
        val connected =
            a.array("providers")
                .filter { it.text("state") == "connected" }
                .mapNotNull { it.text("providerId") }
                .toSet()
        agents = modelChoices(catalog, providers = connected)
        pendingRepository =
            app.vault
                .read("cloud-intent:$identity:repository")
                ?.let(WireJson::parseToJsonElement)
                ?.field("body") ?: JsonNull
        pendingLaunch =
            app.vault
                .read("cloud-intent:$identity:launch")
                ?.let(WireJson::parseToJsonElement)
                ?.field("body") ?: JsonNull
    }

    private suspend fun prepare(operation: String, body: JsonObject): Pair<String, JsonObject> {
        val key = "cloud-intent:${app.account.identity}:$operation"
        activeIntentKey = key
        return key to app.vault.prepareIntent(key, body)
    }

    fun connectRepository(url: String, branch: String, private: Boolean, done: (String) -> Unit) =
        action {
            val identity = app.account.identity
            val (key, request) =
                prepare(
                    "repository",
                    obj(
                        "repositoryUrl" to url.json(),
                        "defaultBranch" to branch.json(),
                        "visibility" to (if (private) "private" else "public").json(),
                    ),
                )
            val result = cloud.request("/v1/cloud/projects", "POST", request)
            check(identity == app.account.identity)
            app.vault.write(key, null)
            loadData()
            done(result.text("projectId") ?: result.field("project").text("projectId").orEmpty())
        }

    fun launch(
        project: JsonElement,
        providerId: String,
        sizeId: String,
        model: ModelChoice,
        done: (CloudChat) -> Unit,
    ) = action {
        val identity = app.account.identity
        val body =
            obj(
                "projectId" to project.field("projectId"),
                "providerId" to providerId.json(),
                "baseRef" to ("origin/" + (project.text("defaultBranch") ?: "main")).json(),
                "agent" to model.provider.json(),
                "model" to model.id.json(),
                "runtimeMode" to "approval-required".json(),
                "sizeId" to sizeId.ifEmpty { null }?.json(),
            )
        val (key, request) = prepare("launch", body)
        val response = cloud.request("/v1/cloud/workspaces", "POST", request)
        check(identity == app.account.identity)
        val workspace = response.field("workspace")
        val workspaceId =
            workspace.text("workspaceId")
                ?: throw ClientException(
                    "Incomplete workspace response. Refresh your cloud list before retrying."
                )
        val sessionId =
            response.text("initialSessionId")
                ?: throw ClientException(
                    "Missing initial chat. Refresh your cloud list before retrying."
                )
        app.vault.write(key, null)
        pendingLaunch = JsonNull
        done(
            CloudChat(
                workspaceId,
                project.text("projectId").orEmpty(),
                response.text("chatId") ?: sessionId,
                sessionId,
                title = "New cloud chat",
                repositoryDisplayName = project.text("displayName").orEmpty(),
                state = workspace.text("state") ?: "starting",
                runtimeState = "starting",
            )
        )
    }

    private suspend fun ensureAuthority() {
        if (auth.text("authorityState") != "ready")
            auth = cloud.request("/v1/cloud/auth/provision", "POST", obj())
    }

    fun login(provider: String) = action {
        ensureAuthority()
        val identity = app.account.identity
        operation =
            cloud.request(
                "/v1/cloud/auth/login/start",
                "POST",
                obj("providerId" to provider.json()),
            )
        polling?.cancel()
        polling =
            viewModelScope.launch {
                try {
                    val id = operation.text("operationId") ?: return@launch
                    while (
                        operation.text("state") == "authorizing" && identity == app.account.identity
                    ) {
                        delay(3000)
                        val next = cloud.request("/v1/cloud/auth/login/${encoded(id)}")
                        if (identity != app.account.identity) return@launch
                        operation = next
                    }
                    if (operation.text("state") == "error")
                        failure = "Agent sign-in failed. Try again."
                    if (identity == app.account.identity) loadData()
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (error: Exception) {
                    failure = error.message
                }
            }
    }

    fun cancelLogin() = action {
        val id = operation.text("operationId") ?: return@action
        cloud.request("/v1/cloud/auth/login/${encoded(id)}/cancel", "POST")
        polling?.cancel()
        operation = JsonNull
        loadData()
    }

    fun configure(provider: String, method: String, secret: String, done: () -> Unit) = action {
        ensureAuthority()
        val keyId =
            auth.text("encryptionKeyId")
                ?: throw ClientException(
                    "Cloud authentication is still starting. Try again shortly."
                )
        val jwk =
            auth.text("encryptionPublicJwk")
                ?: throw ClientException("Missing cloud encryption key.")
        val sealed = withContext(Dispatchers.Default) { CloudSecret.seal(secret, jwk) }
        cloud.request(
            "/v1/cloud/auth/configure",
            "POST",
            obj(
                "providerId" to provider.json(),
                "method" to method.json(),
                "sealedSecret" to obj("keyId" to keyId.json(), "ciphertext" to sealed.json()),
            ),
        )
        done()
        loadData()
    }
}
