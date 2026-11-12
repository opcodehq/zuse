package sh.zuse.android

import android.app.Application
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import androidx.compose.runtime.*
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import java.util.UUID
import kotlinx.coroutines.*
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*
import okhttp3.*
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import sh.zuse.core.*

@Serializable
sealed interface Source {
    @Serializable data class Device(val device: SavedDevice) : Source

    @Serializable data class Cloud(val chat: CloudChat) : Source

    @Serializable data class Computer(val device: RemoteEnvironment) : Source
}

data class Patch(
    val path: String,
    val lines: List<DiffLine>,
    val mode: String,
    val truncated: Boolean,
    val error: String?,
)

class WorkspaceViewModel(application: Application) : AndroidViewModel(application) {
    private val app
        get() = getApplication<ZuseApplication>()

    private val vault
        get() = app.vault

    val account
        get() = app.account

    val projects = mutableStateListOf<Project>()
    val sessions = mutableStateListOf<Session>()
    val messages = mutableStateListOf<Message>()
    val devices = mutableStateListOf<SavedDevice>()
    val cloudChats = mutableStateListOf<CloudChat>()
    val computers = mutableStateListOf<RemoteEnvironment>()
    val archivedSessions = mutableStateListOf<Session>()
    var archivesLoading by mutableStateOf(false)
        private set

    var chatActionBusy by mutableStateOf(false)
        private set

    val canManageChats: Boolean
        get() = connected && rpc?.canonical == true

    val models = mutableStateListOf<ModelChoice>()
    val branches = mutableStateListOf<Branch>()
    val patches = mutableStateListOf<Patch>()
    val pending = mutableStateListOf<PendingMessage>()
    val attachments = mutableStateListOf<JsonElement>()
    val respondedUi = mutableStateListOf<String>()
    val uiDrafts = mutableStateMapOf<String, Map<String, JsonElement>>()
    var interactions by mutableStateOf<List<JsonElement>>(emptyList())
        private set

    var selectedProject by mutableStateOf<Project?>(null)
        private set

    var selectedSession by mutableStateOf<Session?>(null)
        private set

    var source by mutableStateOf<Source?>(null)
        private set

    var status by mutableStateOf("Not connected")
    var linkRoute by mutableStateOf<PairedRouteStatus?>(null)
        private set

    var connected by mutableStateOf(false)
        private set

    var synced by mutableStateOf(false)
        private set

    var signedIn by mutableStateOf(false)
        private set

    var error by mutableStateOf<String?>(null)

    /** The last connection failure, shown quietly while reconnecting rather than as an error. */
    var connectionIssue by mutableStateOf<String?>(null)
        private set

    /** Whether the open computer dropped and is being retried, which is worth a small banner. */
    val reconnecting: Boolean
        get() =
            source != null &&
                !connected &&
                (status.startsWith("Reconnecting") || status.startsWith("Offline"))

    /** A failed new-chat request, shown on the new chat sheet only. */
    var createError by mutableStateOf<String?>(null)
    var busy by mutableStateOf(false)
        private set

    var sending by mutableStateOf(false)
        private set

    var uploading by mutableStateOf(false)
        private set

    var paging by mutableStateOf(false)
        private set

    var cloudLoading by mutableStateOf(false)
        private set

    var diffLoading by mutableStateOf(false)
        private set

    var inputsReady by mutableStateOf(false)
        private set

    var draft by mutableStateOf("")
        private set

    var olderSequence by mutableStateOf<Long?>(null)
        private set

    var activeTurnId by mutableStateOf<String?>(null)
        private set

    var pairingLink by mutableStateOf<String?>(null)

    /** The chat reselected when the app started, which phones should not jump into. */
    var restoredAtLaunch: String? = null
        private set

    private var launching = true
    private var rpc: RpcClient? = null
    private var connectionJob: Job? = null
    private var timelineJob: Job? = null
    private var diffJob: Job? = null
    private var inputJob: Job? = null
    private var draftRevision = 0L
    private var formRevision = 0L
    private var flushRequested = false
    private val requestedRetries = HashSet<String>()
    private var checkpointJob: Job? = null
    private var generation = UUID.randomUUID()
    private var sessionGeneration = UUID.randomUUID()
    private var transcript = Transcript()
    private val rowIndices = HashMap<String, Int>()
    private var foreground = true

    private fun uuid() = UUID.randomUUID().toString()

    private val cloud
        get() = CloudClient(BuildConfig.API_URL, account::token, allowLoopbackHttp = BuildConfig.DEBUG && BuildConfig.DEPLOYMENT_PROFILE == "staging")

    private fun identity(): String =
        when (val source = source) {
            is Source.Device -> "device:${source.device.id}"
            is Source.Cloud -> "${account.identity}|workspace:${source.chat.workspaceId}"
            is Source.Computer -> "${account.identity}|computer:${source.device.environmentId}"
            null -> "disconnected"
        }

    val conversationKey: String
        get() = "${identity()}|${selectedSession?.id.orEmpty()}"

    private fun archiveKey(session: Session) = "${identity()}|${session.id}"

    init {
        action {
            vault.read("devices")?.let {
                devices.addAll(WireJson.decodeFromString<List<SavedDevice>>(it))
            }
            vault.read("new-chat-modes")?.let {
                checkoutModes.putAll(WireJson.decodeFromString<Map<String, String>>(it))
            }
            account.hydrate()
            signedIn = account.signedIn
            if (signedIn) refreshCloud()
            val previous = vault.read("last-source")?.let(WireJson::parseToJsonElement)
            if (previous != null) {
                val target: Source = previous.field("source").decode()
                if (
                    (target is Source.Device && devices.any { it.id == target.device.id }) ||
                        (signedIn && previous.text("account") == account.identity)
                )
                    connect(target)
            }
        }
    }

    private fun action(block: suspend () -> Unit): Job =
        viewModelScope.launch {
            try {
                block()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                error = failure.message ?: "The operation failed. Try again."
            }
        }

    fun handleLink(uri: Uri) {
        if (uri.host == "fixture" && BuildConfig.DEBUG) {
            val address = uri.getQueryParameter("address") ?: return
            require(java.net.URI(address).host in listOf("127.0.0.1", "localhost"))
            connect(Source.Device(SavedDevice(uuid(), "Local test fixture", address, "")))
            return
        }
        if (uri.host == "preview" && BuildConfig.DEBUG) {
            loadPreview(uri.getQueryParameter("large") == "true")
            return
        }
        if (uri.host == "auth")
            action {
                account.callback(uri)
                signedIn = account.signedIn
                refreshCloud()
            }
        else pairingLink = uri.toString()
    }

    private fun loadPreview(large: Boolean) {
        if (!BuildConfig.DEBUG) return
        disconnect()
        val project = Project("preview-project", "zuse")
        val session =
            Session(
                "preview-session",
                project.id,
                "Build something worth using",
                "claude",
                "Sonnet 5.5",
            )
        projects.add(project)
        selectedProject = project
        sessions.add(session)
        selectedSession = session
        status = "Preview · sample data"
        inputsReady = true
        val samples =
            listOf(
                Message(
                    "user",
                    session.id,
                    "user",
                    obj(
                        "_tag" to "text".json(),
                        "text" to "Tighten the chat layout and keep the composer compact.".json(),
                    ),
                ),
                Message(
                    "assistant",
                    session.id,
                    "assistant",
                    obj(
                        "_tag" to "text".json(),
                        "text" to
                            "I’ll follow the mobile app’s spacing, simplify the header, and keep the composer small until you start typing."
                                .json(),
                    ),
                ),
                previewTool("t1", session.id, "readFile", "path", "src/composer.tsx"),
                previewTool("t2", session.id, "search", "pattern", "useComposerHeight"),
                previewTool("t3", session.id, "editFile", "path", "src/composer.tsx"),
                previewTool("t4", session.id, "exec", "command", "pnpm test composer", output = "12 passed"),
                previewTool("t5", session.id, "exec", "command", "pnpm lint", error = true, output = "error  Unexpected any"),
                Message(
                    "result",
                    session.id,
                    "assistant",
                    obj(
                        "_tag" to "text".json(),
                        "text" to
                            "### Updated\n\n- Native controls and keyboard behavior\n- A paged, recycling conversation\n- Your devices, one tap away\n\n```kotlin\nComposer(expanded = focused)\n```\n\nReady to review the changes?"
                                .json(),
                    ),
                ),
            )
        val rows =
            if (large)
                (0 until 10_000).map { index ->
                    Message(
                        "row-$index",
                        session.id,
                        if (index % 3 == 0) "user" else "assistant",
                        obj(
                            "_tag" to "text".json(),
                            "text" to
                                "Message $index · Native transcript performance fixture.".json(),
                        ),
                    )
                }
            else samples
        transcript = Transcript()
        transcript.restore(rows)
        rebuildRows()
    }

    fun signIn(open: (Uri) -> Unit) = action { open(account.authorize()) }

    fun signOut() = action {
        if (source is Source.Cloud || source is Source.Computer) disconnect()
        account.signOut()
        signedIn = false
        cloudChats.clear()
        computers.clear()
    }

    fun refreshCloud() = action {
        if (!signedIn || cloudLoading) return@action
        cloudLoading = true
        try {
            val epoch = account.identity
            supervisorScope {
                val chats = async { runCatching { cloud.chats() } }
                val environments = async { runCatching { cloud.environments() } }
                val loadedChats = chats.await()
                val loadedEnvironments = environments.await()
                if (epoch != account.identity || !signedIn) return@supervisorScope
                loadedChats
                    .onSuccess {
                        cloudChats.clear()
                        cloudChats.addAll(it)
                    }
                    .onFailure { error = it.message }
                loadedEnvironments
                    .onSuccess {
                        computers.clear()
                        computers.addAll(it)
                    }
                    .onFailure { error = it.message }
            }
        } finally {
            cloudLoading = false
        }
    }

    /** Chats on one computer, loaded over a short-lived connection for the home list. */
    data class ComputerInbox(
        val target: Source,
        val label: String,
        val projects: List<Project> = emptyList(),
        val chats: List<Session> = emptyList(),
        /** Null while the first load is in flight. */
        val reachable: Boolean? = null,
    )

    val computerInboxes = mutableStateMapOf<String, ComputerInbox>()
    private var inboxJob: Job? = null
    var inboxRefreshing by mutableStateOf(false)
        private set

    /** The home list's place filter: a place key from [places], or null for every place. */
    var placeFilter by mutableStateOf<String?>(null)

    /** Linked computers already paired directly are listed once, as the paired computer. */
    private val linkedComputers: List<RemoteEnvironment>
        get() {
            val paired = devices.mapNotNull { it.environmentId }.toSet()
            return computers.filter { it.environmentId !in paired }
        }

    private fun placeKey(target: Source): String =
        when (target) {
            is Source.Device -> "device:${target.device.id}"
            is Source.Computer -> "computer:${target.device.environmentId}"
            is Source.Cloud -> CLOUD_PLACE
        }

    private fun placeLabel(target: Source): String =
        when (target) {
            is Source.Device -> target.device.label
            is Source.Computer -> target.device.label ?: "Computer"
            is Source.Cloud -> "Zuse Cloud"
        }

    /** Everywhere a chat can run: paired computers, linked computers and Zuse Cloud. */
    val places: List<Pair<String, String>>
        get() =
            devices.map { "device:${it.id}" to it.label } +
                linkedComputers.map { "computer:${it.environmentId}" to (it.label ?: "Computer") } +
                (if (signedIn) listOf(CLOUD_PLACE to "Zuse Cloud") else emptyList())

    val sourceLabel: String
        get() = source?.let(::placeLabel) ?: "your computer"

    val currentPlace: String?
        get() = source?.let(::placeKey)

    /** Nothing to show chats from yet: no computer paired and not signed in. */
    val unconfigured: Boolean
        get() =
            devices.isEmpty() &&
                computers.isEmpty() &&
                !signedIn &&
                source == null &&
                !(BuildConfig.DEBUG && status.startsWith("Preview"))

    val activePlaceFilter: String?
        get() = placeFilter?.takeIf { key -> places.any { it.first == key } }

    /** Every chat the home list can show, from all computers and Zuse Cloud. */
    fun chatRows(): List<ChatRow> {
        val rows = mutableListOf<ChatRow>()
        fun add(target: Source, projects: List<Project>, chats: List<Session>) {
            val key = placeKey(target)
            val label = placeLabel(target)
            chats.forEach { chat ->
                val project =
                    projects
                        .firstOrNull { it.id == chat.projectId }
                        ?.takeIf { it.id != CanonicalWorkspace.PROJECTLESS }
                rows +=
                    ChatRow(
                        "$key|${chat.id}",
                        chat.id,
                        key,
                        label,
                        chat.projectId,
                        project?.name,
                        chat.title,
                        chat.preview,
                        ChatStatus.from(chat.status),
                        chat.branch,
                        ChatList.parseTime(chat.activityAt),
                    )
            }
        }
        computerInboxes.values.forEach { add(it.target, it.projects, it.chats) }
        val current = source
        if (current != null && current !is Source.Cloud && computerInboxes.values.none { it.target == current })
            add(current, projects, sessions)
        if (current == null && BuildConfig.DEBUG && status.startsWith("Preview"))
            add(Source.Device(SavedDevice("preview", "Demo computer", "", "")), projects, sessions)
        cloudChats.forEach { chat ->
            rows +=
                ChatRow(
                    "$CLOUD_PLACE|${chat.chatId}",
                    chat.chatId,
                    CLOUD_PLACE,
                    "Zuse Cloud",
                    chat.projectId,
                    chat.repositoryDisplayName,
                    chat.title,
                    status = ChatStatus.from(chat.runtimeState),
                )
        }
        return rows
    }

    fun isOpen(row: ChatRow): Boolean {
        val current = source
        return when {
            row.placeKey == CLOUD_PLACE -> (current as? Source.Cloud)?.chat?.chatId == row.chatId
            current == null -> selectedSession?.id == row.chatId
            else ->
                selectedSession?.id == row.chatId &&
                    (row.placeKey == placeKey(current) ||
                        computerInboxes.values.none { it.target == current })
        }
    }

    /** Opens a home-list row on its computer, connecting to that computer when needed. */
    fun openChat(row: ChatRow) {
        newChatOpen = false
        if (row.placeKey == CLOUD_PLACE) {
            cloudChats.firstOrNull { it.chatId == row.chatId }?.let { connect(Source.Cloud(it)) }
            return
        }
        val inbox = computerInboxes[row.placeKey]
        val session =
            inbox?.chats?.firstOrNull { it.id == row.chatId }
                ?: sessions.firstOrNull { it.id == row.chatId }
                ?: return
        if (inbox == null) {
            selectedProject = projects.firstOrNull { it.id == session.projectId }
            selectSession(session)
        } else openInboxChat(inbox.target, session)
    }

    /** A new chat being written in the chat screen; nothing exists on a host until it is sent. */
    var newChatOpen by mutableStateOf(false)
        private set

    /** The first message of the new chat, kept when the new chat is left unsent. */
    var newChatPrompt by mutableStateOf("")

    /** Ask the chat composer to take focus once, so a just-created chat keeps the keyboard up. */
    var focusComposer by mutableStateOf(false)

    fun openNewChat() {
        prepareNewChat()
        createError = null
        newChatOpen = true
    }

    fun closeNewChat() {
        newChatOpen = false
        createError = null
    }

    /**
     * Where a new chat should go: the filtered place, else the current computer when reachable,
     * else the first computer that is.
     */
    fun prepareNewChat() {
        val reachable = { key: String -> computerInboxes[key]?.reachable != false }
        val computersFirst = places.map { it.first }.filter { it != CLOUD_PLACE }
        val current = source?.takeIf { it !is Source.Cloud }?.let(::placeKey)
        val wanted =
            activePlaceFilter?.takeIf { it != CLOUD_PLACE }
                ?: current?.takeIf(reachable)
                ?: computersFirst.firstOrNull(reachable)
                ?: current
                ?: computersFirst.firstOrNull()
                ?: return
        if (current == wanted) return
        targetFor(wanted)?.let { connect(it) }
    }

    fun targetFor(placeKey: String): Source? =
        devices.firstOrNull { "device:${it.id}" == placeKey }?.let { Source.Device(it) }
            ?: linkedComputers
                .firstOrNull { "computer:${it.environmentId}" == placeKey }
                ?.let { Source.Computer(it) }

    fun openInboxChat(target: Source, session: Session) {
        if (source == target && connected) {
            selectedProject = projects.firstOrNull { it.id == session.projectId }
            selectSession(session)
        } else connect(target, initialSession = session)
    }

    /** Places whose chats could not be loaded on the last refresh. */
    val unreachablePlaces: List<String>
        get() =
            computerInboxes
                .filter { (key, inbox) ->
                    inbox.reachable == false && activePlaceFilter.let { it == null || it == key }
                }
                .values
                .map { it.label }

    fun refreshHome() {
        refreshComputerInboxes()
        if (signedIn) refreshCloud()
    }

    /** Reloads every computer's chats; [quiet] refreshes in place without a progress indicator. */
    fun refreshComputerInboxes(quiet: Boolean = false) {
        inboxJob?.cancel()
        val targets =
            devices.map { "device:${it.id}" to ComputerInbox(Source.Device(it), it.label) } +
                linkedComputers.map {
                    "computer:${it.environmentId}" to
                        ComputerInbox(Source.Computer(it), it.label ?: "Computer")
                }
        computerInboxes.keys.retainAll(targets.map { it.first }.toSet())
        targets.forEach { (key, value) ->
            if (key !in computerInboxes) computerInboxes[key] = value
        }
        if (targets.isEmpty()) {
            inboxRefreshing = false
            return
        }
        if (!quiet) inboxRefreshing = true
        val job =
            viewModelScope.launch {
                val limit = kotlinx.coroutines.sync.Semaphore(3)
                supervisorScope {
                    targets.forEach { (key, row) ->
                        launch {
                            limit.acquire()
                            var client: RpcClient? = null
                            try {
                                val (projects, chats) =
                                    withTimeout(12_000) {
                                        client =
                                            when (val target = row.target) {
                                                is Source.Device ->
                                                    RpcClient(
                                                            target.device.address,
                                                            token = target.device.token,
                                                            http =
                                                                pinnedHttp(
                                                                    target.device.certificatePin
                                                                ),
                                                            pairingLink = target.device.pairingLink,
                                                        )
                                                        .also {
                                                            client = it
                                                            it.connect()
                                                            target.device.environmentId?.let {
                                                                expected ->
                                                                check(
                                                                    it.call(
                                                                            if (it.canonical)
                                                                                "EngineInfo"
                                                                            else "connect.describe"
                                                                        )
                                                                        .text(
                                                                            if (it.canonical)
                                                                                "deviceId"
                                                                            else "environmentId"
                                                                        ) == expected
                                                                )
                                                            }
                                                        }
                                                is Source.Computer ->
                                                    cloud.connectEnvironment(
                                                        target.device.environmentId,
                                                        DeviceProof()::sign,
                                                        target.device.canonicalAccount,
                                                    )
                                                else -> error("Invalid computer")
                                            }
                                        val active = requireNotNull(client)
                                        if (active.canonical) {
                                            val workspace = CanonicalWorkspace(active)
                                            val projects = workspace.projects()
                                            val live = workspace.runtimeStatuses()
                                            projects to
                                                workspace.allSessions().map {
                                                    it.copy(status = live[it.id])
                                                }
                                        } else {
                                            val projects =
                                                active.call("workspace.list").decode<List<Project>>()
                                            projects to
                                                projects.flatMap {
                                                    active
                                                        .call(
                                                            "session.list",
                                                            obj("projectId" to it.id.json()),
                                                        )
                                                        .decode<List<Session>>()
                                                }
                                        }
                                    }
                                computerInboxes[key] =
                                    row.copy(projects = projects, chats = chats, reachable = true)
                                // Titles are named by the host after the first reply.
                                val open = selectedSession
                                if (row.target == source && open != null)
                                    chats
                                        .firstOrNull { it.id == open.id && it.title != open.title }
                                        ?.let { selectedSession = open.copy(title = it.title) }
                            } catch (cancelled: CancellationException) {
                                if (cancelled is TimeoutCancellationException)
                                    computerInboxes[key] =
                                        (computerInboxes[key] ?: row).copy(reachable = false)
                                else throw cancelled
                            } catch (_: Throwable) {
                                computerInboxes[key] =
                                    (computerInboxes[key] ?: row).copy(reachable = false)
                            } finally {
                                client?.close()
                                limit.release()
                            }
                        }
                    }
                }
            }
        inboxJob = job
        job.invokeOnCompletion { viewModelScope.launch { if (inboxJob === job) inboxRefreshing = false } }
    }

    fun connect(next: Source, initialSession: Session? = null) {
        error = null
        connectionIssue = null
        // Opening a particular chat leaves the new chat; switching computers for it does not.
        if (initialSession != null || next is Source.Cloud) newChatOpen = false
        archivedSessions.clear()
        archivesLoading = false
        persistDraft()
        connectionJob?.cancel()
        timelineJob?.cancel()
        diffJob?.cancel()
        checkpointJob?.cancel()
        rpc?.close()
        rpc = null
        generation = UUID.randomUUID()
        sessionGeneration = UUID.randomUUID()
        source = next
        val sourceRecord =
            obj(
                "source" to WireJson.encodeToJsonElement<Source>(next),
                "account" to account.identity.json(),
            )
        action { vault.write("last-source", sourceRecord.toString()) }
        connected = false
        synced = false
        projects.clear()
        sessions.clear()
        selectedProject = null
        selectedSession = initialSession
        inputsReady = false
        messages.clear()
        rowIndices.clear()
        transcript = Transcript()
        pending.clear()
        attachments.clear()
        respondedUi.clear()
        uiDrafts.clear()
        draft = ""
        interactions = emptyList()
        activeTurnId = null
        startConnection()
    }

    private fun startConnection() {
        connectionJob?.cancel()
        val current = generation
        connectionJob =
            viewModelScope.launch {
                val backoff = RetryBackoff()
                try {
                    val previous =
                        vault.read("last-session:${identity()}")?.let {
                            WireJson.decodeFromString<Session>(it)
                        }
                    val target = source
                    if (
                        current == generation &&
                            selectedSession == null &&
                            previous != null &&
                            (target !is Source.Cloud || target.chat.sessionId == previous.id)
                    ) {
                        // Reopened at launch: selected for tablets, but phones start on the list.
                        if (launching) restoredAtLaunch = previous.id
                        selectSession(previous)
                    }
                } catch (failure: Exception) {
                    error = "Could not restore the previous chat: ${failure.message}"
                }
                launching = false
                while (isActive && foreground && current == generation) {
                    var client: RpcClient? = null
                    var readyAt: Long? = null
                    try {
                        connected = false
                        synced = false
                        status = if (backoff.attempt == 0) "Connecting…" else "Reconnecting…"
                        linkRoute = null
                        client =
                            when (val target = source) {
                                is Source.Device ->
                                    RpcClient(
                                            target.device.address,
                                            token = target.device.token,
                                            http = pinnedHttp(target.device.certificatePin),
                                            pairingLink = target.device.pairingLink,
                                            onRoute = { route ->
                                                viewModelScope.launch { linkRoute = route }
                                            },
                                        )
                                        .also {
                                            it.connect()
                                            try {
                                                target.device.environmentId?.let { expected ->
                                                    require(
                                                        it.call(
                                                                if (it.canonical) "EngineInfo"
                                                                else "connect.describe"
                                                            )
                                                            .text(
                                                                if (it.canonical) "deviceId"
                                                                else "environmentId"
                                                            ) == expected
                                                    ) {
                                                        "The computer identity changed. Pair again."
                                                    }
                                                }
                                            } catch (failure: Throwable) {
                                                it.close()
                                                throw failure
                                            }
                                        }
                                is Source.Cloud -> cloud.connect(target.chat.workspaceId)
                                is Source.Computer ->
                                    cloud.connectEnvironment(
                                        target.device.environmentId,
                                        withContext(Dispatchers.IO) { DeviceProof() }::sign,
                                        canonicalAccount = target.device.canonicalAccount,
                                    )
                                null -> return@launch
                            }
                        ensureActive()
                        if (current != generation) return@launch
                        rpc = client
                        val rows: List<Project> =
                            if (client.canonical) CanonicalWorkspace(client).projects()
                            else client.call("workspace.list").decode()
                        ensureActive()
                        projects.clear()
                        projects.addAll(rows)
                        connected = true
                        error = null
                        connectionIssue = null
                        status = "Connected"
                        val selected = selectedSession
                        if (selected != null) {
                            selectedProject = projects.find { it.id == selected.projectId }
                            if (client.canonical) sessions.clear()
                            loadSessions()
                            val restored =
                                if (client.canonical) sessions.firstOrNull { it.id == selected.id }
                                else selected
                            if (restored != null) selectSession(restored)
                            else {
                                selectedSession = null
                                messages.clear()
                                rowIndices.clear()
                                pending.clear()
                                attachments.clear()
                                interactions = emptyList()
                                draft = ""
                                vault.write("last-session:${identity()}", null)
                                error =
                                    "The previous chat was archived or removed. Choose another chat."
                            }
                        } else if (source is Source.Cloud) {
                            val session: Session =
                                if (client.canonical)
                                    CanonicalWorkspace(client)
                                        .session((source as Source.Cloud).chat.sessionId)
                                else
                                    client
                                        .call(
                                            "session.get",
                                            obj(
                                                "sessionId" to
                                                    (source as Source.Cloud).chat.sessionId.json()
                                            ),
                                        )
                                        .decode()
                            selectedProject = projects.find { it.id == session.projectId }
                            loadSessions()
                            selectSession(session)
                        } else {
                            selectedProject = projects.firstOrNull()
                            loadSessions()
                        }
                        loadModels()
                        readyAt = System.nanoTime()
                        throw client.awaitDisconnect()
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (failure: Throwable) {
                        if (current != generation) return@launch
                        status = "Offline · retrying"
                        connected = false
                        synced = false
                        // The banner shows reconnection; a snackbar per retry is only noise.
                        connectionIssue = failure.message
                        timelineJob?.cancel()
                        client?.close()
                        rpc = null
                        val healthyFor = readyAt?.let { (System.nanoTime() - it) / 1_000_000 } ?: 0L
                        delay(backoff.nextDelay(healthyFor))
                    } finally {
                        if (!isActive || current != generation) client?.close()
                    }
                }
            }
    }

    fun reconnect() {
        if (source != null) {
            rpc?.close()
            startConnection()
        }
    }

    fun setForeground(value: Boolean) {
        if (foreground == value) return
        foreground = value
        if (!value) {
            persistDraft()
            connectionJob?.cancel()
            timelineJob?.cancel()
            rpc?.close()
            rpc = null
            connected = false
            synced = false
            status = "Paused"
        } else if (source != null) startConnection()
    }

    private fun previewTool(
        id: String,
        session: String,
        kind: String,
        field: String,
        value: String,
        resolved: Boolean = true,
        error: Boolean = false,
        output: String? = null,
    ) =
        Message(
            id,
            session,
            "assistant",
            obj(
                "_tag" to "tool_use".json(),
                "tool" to kind.json(),
                "input" to obj("kind" to kind.json(), field to value.json()),
                "resolved" to resolved.json(),
                "isError" to error.json(),
                "output" to output?.json(),
            ),
        )

    /** Where a message from the composer will run. */
    val composerContext: String?
        get() =
            projects
                .firstOrNull {
                    it.id == selectedSession?.projectId &&
                        it.id != CanonicalWorkspace.PROJECTLESS
                }
                ?.name
                ?.takeIf { it.isNotBlank() }

    /** The route in use, for the computer this workspace is connected to. */
    fun linkSummary(device: SavedDevice): String? {
        val route = linkRoute ?: return null
        val label = route.label ?: return null
        if (!connected || (source as? Source.Device)?.device?.id != device.id) return null
        return route.rttMs?.let { "$label · $it ms" } ?: label
    }

    fun networkRecovered() {
        if (foreground && source != null && !connected) reconnect()
    }

    fun disconnect() {
        archivedSessions.clear()
        archivesLoading = false
        action { vault.write("last-source", null) }
        persistDraft()
        generation = UUID.randomUUID()
        sessionGeneration = UUID.randomUUID()
        connectionJob?.cancel()
        timelineJob?.cancel()
        diffJob?.cancel()
        rpc?.close()
        rpc = null
        source = null
        connected = false
        synced = false
        status = "Not connected"
        projects.clear()
        sessions.clear()
        messages.clear()
        rowIndices.clear()
        pending.clear()
        attachments.clear()
        respondedUi.clear()
        uiDrafts.clear()
        interactions = emptyList()
        selectedProject = null
        selectedSession = null
        inputsReady = false
        draft = ""
        activeTurnId = null
    }

    fun selectProject(project: Project) = action {
        selectedProject = project
        sessions.clear()
        loadSessions()
    }

    fun loadArchivedChats() = action {
        val client = rpc?.takeIf { it.canonical } ?: return@action
        val epoch = generation
        archivesLoading = true
        try {
            val rows = CanonicalWorkspace(client).archivedSessions()
            if (epoch == generation) {
                archivedSessions.clear()
                archivedSessions.addAll(rows)
            }
        } finally {
            if (epoch == generation) archivesLoading = false
        }
    }

    private fun chatAction(block: suspend () -> Unit) = action {
        if (chatActionBusy) return@action
        chatActionBusy = true
        try {
            block()
        } finally {
            chatActionBusy = false
        }
    }

    fun renameChat(session: Session, title: String, done: () -> Unit) = chatAction {
        val client =
            rpc?.takeIf { it.canonical } ?: throw ClientException("Reconnect to rename this chat.")
        val epoch = generation
        CanonicalWorkspace(client).rename(session.id, title)
        if (epoch != generation) return@chatAction
        if (selectedSession?.id == session.id)
            selectedSession = selectedSession?.copy(title = title.trim())
        loadSessions()
        done()
    }

    fun setChatArchived(session: Session, archived: Boolean, done: () -> Unit = {}) = chatAction {
        val client =
            rpc?.takeIf { it.canonical } ?: throw ClientException("Reconnect to manage this chat.")
        val epoch = generation
        persistDraft()
        CanonicalWorkspace(client).setArchived(session.id, archived)
        if (epoch != generation) return@chatAction
        if (archived && selectedSession?.id == session.id) {
            vault.write("last-session:${identity()}", null)
            timelineJob?.cancel()
            checkpointJob?.cancel()
            inputJob?.cancel()
            diffJob?.cancel()
            sessionGeneration = UUID.randomUUID()
            selectedSession = null
            synced = false
            inputsReady = false
            messages.clear()
            rowIndices.clear()
            pending.clear()
            attachments.clear()
            interactions = emptyList()
            draft = ""
            activeTurnId = null
        }
        loadSessions()
        archivedSessions.removeAll { it.id == session.id }
        if (archived) loadArchivedChats()
        done()
    }

    private suspend fun loadSessions() {
        val project = selectedProject ?: return
        val epoch = generation
        val rows: List<Session> =
            rpc?.let {
                if (it.canonical) CanonicalWorkspace(it).sessions(project.id)
                else
                    it.call("session.list", obj("projectId" to project.id.json()))
                        .decode<List<Session>>()
            } ?: return
        if (epoch == generation && selectedProject?.id == project.id) {
            sessions.clear()
            sessions.addAll(rows)
            computerInboxes.entries
                .firstOrNull { it.value.target == source }
                ?.let { entry ->
                    computerInboxes[entry.key] =
                        entry.value.copy(
                            chats =
                                (entry.value.chats.filterNot { it.projectId == project.id } +
                                        rows.map { row ->
                                            row.copy(
                                                status =
                                                    entry.value.chats
                                                        .firstOrNull { it.id == row.id }
                                                        ?.status
                                            )
                                        })
                                    .distinctBy { it.id },
                            projects = projects.toList(),
                            reachable = true,
                        )
                }
        }
    }

    fun selectSession(session: Session) {
        persistDraft()
        timelineJob?.cancel()
        checkpointJob?.cancel()
        sessionGeneration = UUID.randomUUID()
        val current = sessionGeneration
        val same = selectedSession?.id == session.id
        selectedSession = session
        inputsReady = false
        synced = false
        olderSequence = null
        activeTurnId = null
        interactions = emptyList()
        paging = false
        sending = false
        uploading = false
        val key = archiveKey(session)
        val metadataKey = "last-session:${identity()}"
        action { vault.write(metadataKey, WireJson.encodeToString(session)) }
        if (!same) {
            draftRevision = 0
            formRevision = 0
            requestedRetries.clear()
            transcript = Transcript()
            messages.clear()
            rowIndices.clear()
            draft = ""
            pending.clear()
            attachments.clear()
            respondedUi.clear()
            uiDrafts.clear()
        }
        timelineJob =
            viewModelScope.launch {
                try {
                    val metadata = vault.readMetadata(key)
                    ensureActive()
                    if (!same || metadata.draftRevision > draftRevision) {
                        draft = metadata.draft
                        draftRevision = metadata.draftRevision
                    }
                    if (!same || metadata.formRevision > formRevision) {
                        uiDrafts.clear()
                        uiDrafts.putAll(metadata.uiDrafts)
                        formRevision = metadata.formRevision
                    }
                    pending.clear()
                    pending.addAll(metadata.pending)
                    attachments.clear()
                    attachments.addAll(metadata.attachments)
                    respondedUi.clear()
                    respondedUi.addAll(metadata.responses.keys)
                    inputsReady = true
                    if (!same) {
                        val history = vault.readHistory(key)
                        ensureActive()
                        transcript.restore(history)
                        rebuildRows()
                    }
                    val client = rpc ?: return@launch
                    if (client.canonical) {
                        coroutineScope {
                            var runtimeRow: JsonElement? = null
                            fun publishRuntime() {
                                val live = CanonicalRuntime.status(runtimeRow, session.id)
                                activeTurnId =
                                    if (live in listOf("working", "awaitingInput"))
                                        "canonical:${session.id}"
                                    else null
                                if (synced)
                                    status =
                                        when (live) {
                                            "working" -> "Working"
                                            "awaitingInput" -> "Waiting for your answer"
                                            "errored" -> "Run failed"
                                            null -> "Connected · status unavailable"
                                            else -> "Connected"
                                        }
                            }
                            launch {
                                while (isActive) {
                                    delay(5000)
                                    check(current == sessionGeneration)
                                    publishRuntime()
                                }
                            }

                            launch {
                                client.stream("WatchSessions").collect { frame ->
                                    ensureActive()
                                    check(current == sessionGeneration)
                                    runtimeRow =
                                        (frame as? JsonArray)?.firstOrNull {
                                            it.text("chatId") == session.id
                                        }
                                    publishRuntime()
                                }
                                throw ClientException("Runtime status stream ended. Reconnecting…")
                            }
                            val canonicalTranscript = CanonicalTranscript()
                            client
                                .stream(
                                    "WatchDocMessages",
                                    obj("chatId" to session.id.json(), "openingTail" to true.json()),
                                )
                                .collect { update ->
                                    ensureActive()
                                    check(current == sessionGeneration)
                                    val (rows, inputs) =
                                        withContext(Dispatchers.Default) {
                                            canonicalTranscript.apply(update)
                                            canonicalTranscript.messages(session.id) to
                                                canonicalTranscript.interactions()
                                        }
                                    ensureActive()
                                    check(current == sessionGeneration)
                                    interactions = inputs
                                    transcript.replace(rows)
                                    // Keep unchanged Compose rows stable during live deltas.
                                    if (messages.map { it.id } == rows.map { it.id }) {
                                        rows.forEachIndexed { index, row ->
                                            if (messages[index] != row) messages[index] = row
                                        }
                                    } else rebuildRows()
                                    if (!update.flag("historyPending")) {
                                        synced = true
                                        status = "Connected"
                                        flushOutbox()
                                    }
                                    scheduleCheckpoint(key, current)
                                }
                            throw ClientException("Transcript stream ended. Reconnecting…")
                        }
                    }
                    client
                        .stream(
                            "session.events",
                            obj("sessionId" to session.id.json(), "includeRuntime" to true.json()),
                        )
                        .collect { frame ->
                            ensureActive()
                            check(current == sessionGeneration)
                            transcript.apply(frame, session.id)
                            publishFrame(frame)
                            if (frame.text("kind") == "synchronized") {
                                synced = true
                                status = "Connected"
                                flushOutbox()
                            }
                            scheduleCheckpoint(key, current)
                        }
                    throw ClientException("Transcript stream ended. Reconnecting…")
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Throwable) {
                    if (current == sessionGeneration) {
                        synced = false
                        connectionIssue = failure.message
                        rpc?.close()
                    }
                }
            }
    }

    private fun publishFrame(frame: JsonElement) {
        val event = frame.field("event")
        if (frame.text("kind") == "event" && event.text("_tag") == "MessagePersisted") {
            val id = event.field("message").text("id")
            val index = rowIndices[id]
            if (index != null) messages[index] = transcript.messages[index]
            else if (transcript.messages.size > messages.size) {
                val row = transcript.messages.last()
                rowIndices[row.id] = messages.size
                messages.add(row)
            }
        } else if (
            frame.text("kind") in listOf("snapshot", "snapshot-chunk") ||
                event.text("_tag") == "TranscriptRolledBack"
        )
            rebuildRows()
        olderSequence = transcript.olderSequence
        activeTurnId = transcript.activeTurnId
        interactions = transcript.interactions
    }

    private fun rebuildRows() {
        messages.clear()
        messages.addAll(transcript.messages)
        rowIndices.clear()
        messages.forEachIndexed { index, row -> rowIndices[row.id] = index }
    }

    private fun scheduleCheckpoint(key: String, current: UUID) {
        if (checkpointJob?.isActive == true) return
        checkpointJob =
            viewModelScope.launch {
                delay(1000)
                if (current == sessionGeneration) {
                    val history = transcript.messages.toList()
                    try {
                        vault.saveHistory(key, history)
                    } catch (failure: Exception) {
                        error = "Could not save chat history: ${failure.message}"
                    }
                }
            }
    }

    private fun inputsSnapshot() =
        SessionInputs(draftRevision, draft, formRevision, uiDrafts.toMap())

    private fun scheduleInputs() {
        inputJob?.cancel()
        val key = selectedSession?.let(::archiveKey) ?: return
        val inputs = inputsSnapshot()
        inputJob = action {
            delay(250)
            vault.updateMetadata(key) { it.withInputs(inputs) }
        }
    }

    fun updateDraft(value: String) {
        if (!inputsReady || draft == value) return
        draft = value
        draftRevision++
        scheduleInputs()
    }

    private fun persistDraft() {
        inputJob?.cancel()
        if (!inputsReady) return
        val key = selectedSession?.let(::archiveKey) ?: return
        val inputs = inputsSnapshot()
        action { vault.updateMetadata(key) { it.withInputs(inputs) } }
    }

    fun updateUiDraft(message: Message, name: String, value: JsonElement) {
        val session = selectedSession ?: return
        if (!inputsReady || message.sessionId != session.id) return
        uiDrafts[message.id] = uiDrafts[message.id].orEmpty() + (name to value)
        formRevision++
        scheduleInputs()
    }

    fun loadOlder() = action {
        val session = selectedSession ?: return@action
        val before = olderSequence ?: return@action
        val client = rpc ?: return@action
        if (paging) return@action
        val current = sessionGeneration
        paging = true
        try {
            val page =
                client.call(
                    "session.messages.page",
                    obj(
                        "sessionId" to session.id.json(),
                        "beforeSequence" to before.json(),
                        "limit" to 100.json(),
                    ),
                )
            if (current == sessionGeneration) {
                transcript.prependPage(page, session.id)
                rebuildRows()
                olderSequence = transcript.olderSequence
            }
        } finally {
            if (current == sessionGeneration) paging = false
        }
    }

    fun send() = action {
        val session = selectedSession ?: return@action
        if (!inputsReady || sending || uploading || (draft.isBlank() && attachments.isEmpty()))
            return@action
        val current = sessionGeneration
        val key = archiveKey(session)
        val text = draft
        val refs = attachments.toList()
        sending = true
        inputJob?.cancel()
        draftRevision++
        val submittedInputs = inputsSnapshot().copy(draft = "")
        try {
            val item = PendingMessage(uuid(), uuid(), session.id, text.trim(), attachments = refs)
            val archive =
                vault.updateMetadata(key) {
                    it.copy(
                            attachments = it.attachments.filterNot { ref -> ref in refs },
                            pending = it.pending + item,
                        )
                        .withInputs(submittedInputs)
                }
            if (current != sessionGeneration) return@action
            if (draftRevision == submittedInputs.draftRevision) draft = ""
            attachments.removeAll(refs.toSet())
            pending.clear()
            pending.addAll(archive.pending)
        } finally {
            if (current == sessionGeneration) sending = false
        }
        if (current == sessionGeneration) flushOutbox()
    }

    fun flushOutbox(retryId: String? = null) = action {
        if (retryId != null) requestedRetries.add(retryId)
        if (sending) {
            flushRequested = true
            return@action
        }
        if (!synced) return@action
        val session = selectedSession ?: return@action
        val key = archiveKey(session)
        val client = rpc ?: return@action
        val current = sessionGeneration
        sending = true
        try {
            do {
                flushRequested = false
                val queued = vault.readMetadata(key).pending
                if (current != sessionGeneration || !synced) break
                pending.clear()
                pending.addAll(queued)
                for (item in queued) {
                    if (current != sessionGeneration || !synced) break
                    val retry = requestedRetries.remove(item.id)
                    if ((item.attempted || item.rejected != null) && !retry) continue
                    val archive =
                        vault.updateMetadata(key) {
                            it.copy(
                                pending =
                                    it.pending.map { old ->
                                        if (old.id == item.id)
                                            old.copy(attempted = true, rejected = null)
                                        else old
                                    }
                            )
                        }
                    if (current != sessionGeneration) break
                    pending.clear()
                    pending.addAll(archive.pending)
                    try {
                        if (client.canonical) CanonicalWorkspace(client).send(item)
                        else client.call("messages.send", item.payload)
                    } catch (failure: Exception) {
                        if (failure is CancellationException || RequestOutcome.uncertain(failure))
                            throw failure
                        // Refused before it ran: not uncertain, so it can be retried as is.
                        val refused =
                            vault.updateMetadata(key) {
                                it.copy(
                                    pending =
                                        it.pending.map { old ->
                                            if (old.id == item.id)
                                                old.copy(
                                                    attempted = false,
                                                    rejected =
                                                        failure.message ?: "The message was not sent.",
                                                )
                                            else old
                                        }
                                )
                            }
                        if (current == sessionGeneration) {
                            pending.clear()
                            pending.addAll(refused.pending)
                        }
                        // Later messages wait so they never arrive ahead of this one.
                        flushRequested = false
                        break
                    }
                    val updated =
                        vault.updateMetadata(key) {
                            it.copy(pending = it.pending.filterNot { p -> p.id == item.id })
                        }
                    if (current == sessionGeneration) {
                        pending.clear()
                        pending.addAll(updated.pending)
                    }
                }
            } while (flushRequested && current == sessionGeneration && synced)
        } finally {
            if (current == sessionGeneration) sending = false
        }
    }

    suspend fun respondToUi(message: Message, text: String) {
        val session = selectedSession ?: throw ClientException("Open this chat to respond.")
        require(inputsReady && session.id == message.sessionId && source != null) {
            "Open this chat on its connected device to respond."
        }
        require(text.length <= 64 * 1024) { "This response is too large." }
        val current = sessionGeneration
        val item = PendingMessage(uuid(), uuid(), session.id, text)
        formRevision++
        val submittedInputs = inputsSnapshot().copy(uiDrafts = uiDrafts.toMap() - message.id)
        val updated =
            vault.updateMetadata(archiveKey(session)) { record ->
                if (record.responses.containsKey(message.id)) record
                else
                    record
                        .copy(
                            pending = record.pending + item,
                            responses = record.responses + (message.id to item.id),
                        )
                        .withInputs(submittedInputs)
            }
        if (current == sessionGeneration) {
            pending.clear()
            pending.addAll(updated.pending)
            respondedUi.clear()
            respondedUi.addAll(updated.responses.keys)
            uiDrafts.remove(message.id)
            flushOutbox()
        }
    }

    fun removePending(id: String) = action {
        if (sending) return@action
        val key = selectedSession?.let(::archiveKey) ?: return@action
        val current = sessionGeneration
        val updated =
            vault.updateMetadata(key) {
                it.copy(pending = it.pending.filterNot { p -> p.id == id })
            }
        if (current == sessionGeneration) {
            pending.clear()
            pending.addAll(updated.pending)
            requestedRetries.remove(id)
            flushOutbox()
        }
    }

    fun changeModel(model: ModelChoice) = action {
        val session = selectedSession ?: return@action
        val current = sessionGeneration
        rpc?.takeIf { it.canonical }
            ?.let {
                CanonicalWorkspace(it).changeModel(session.id, model.provider, model.id)
                if (current == sessionGeneration)
                    selectedSession = session.copy(providerId = model.provider, model = model.id)
                return@action
            }
        val method =
            if (model.provider == session.providerId) "session.setModel" else "session.setProvider"
        rpc?.call(
            method,
            obj(
                "sessionId" to session.id.json(),
                "model" to model.id.json(),
                "providerId" to
                    if (model.provider != session.providerId) model.provider.json() else null,
            ),
        ) ?: return@action
        if (current == sessionGeneration)
            selectedSession = session.copy(providerId = model.provider, model = model.id)
    }

    suspend fun readFile(path: String): List<String> {
        val session = selectedSession ?: throw ClientException("Open a chat first.")
        val epoch = generation
        val selection = sessionGeneration
        rpc?.takeIf { it.canonical }
            ?.let { client ->
                val file =
                    client.call(
                        "ReadWorkspaceFile",
                        obj("chatId" to session.id.json(), "path" to path.json()),
                    )
                val text =
                    file.text("text")
                        ?: throw ClientException("This file has no readable text content.")
                check(epoch == generation && selection == sessionGeneration) { "The selected chat changed." }
                if (file.flag("truncated")) error = "Showing the host's truncated file preview."
                val lines = withContext(Dispatchers.Default) { text.split('\n') }
                check(epoch == generation && selection == sessionGeneration) { "The selected chat changed." }
                return lines
            }
        val result =
            rpc?.call(
                "git.reviewFileContents",
                obj(
                    "folderId" to session.projectId.json(),
                    "path" to path.json(),
                    "worktreeId" to session.worktreeId?.json(),
                ),
            ) ?: throw ClientException("Reconnect to view this file.")
        val content =
            result.text("newContent")
                ?: throw ClientException("This file was removed or has no text content.")
        check(epoch == generation && selection == sessionGeneration) { "The selected chat changed." }
        val lines = withContext(Dispatchers.Default) { content.split('\n') }
        check(epoch == generation && selection == sessionGeneration) { "The selected chat changed." }
        return lines
    }

    suspend fun listDirectory(directory: String, cursor: String? = null): JsonElement {
        val session = selectedSession ?: throw ClientException("Open a chat first.")
        val client = rpc ?: throw ClientException("Reconnect to browse files.")
        require(client.canonical) { "File browsing requires the Rust host." }
        val epoch = generation
        val result = client.call("ListWorkspaceDirectory", obj("chatId" to session.id.json(), "directory" to directory.json(), "cursor" to cursor?.json()))
        check(epoch == generation && selectedSession?.id == session.id) { "The selected chat changed." }
        return result
    }

    fun stop() = action {
        val session = selectedSession ?: return@action
        rpc?.takeIf { it.canonical }
            ?.let {
                CanonicalWorkspace(it).stop(session.id)
                return@action
            }
        val turn = activeTurnId ?: return@action
        rpc?.call(
            "messages.interrupt",
            obj(
                "sessionId" to session.id.json(),
                "commandId" to uuid().json(),
                "expectedTurnId" to turn.json(),
            ),
        )
    }

    private suspend fun loadModels() {
        val epoch = generation
        val catalog =
            rpc?.let {
                if (it.canonical) CanonicalWorkspace(it).catalog() else it.call("model.catalog")
            } ?: return
        if (epoch != generation) return
        val options = modelChoices(catalog, availableOnly = true)
        models.clear()
        models.addAll(options)
    }

    fun loadBranches() = action {
        val project = selectedProject ?: return@action
        val epoch = generation
        val result: List<Branch> =
            rpc?.let {
                if (it.canonical) CanonicalWorkspace(it).branches(project.id)
                else
                    it.call("git.branches", obj("folderId" to project.id.json()))
                        .decode<List<Branch>>()
            } ?: return@action
        if (epoch == generation && selectedProject?.id == project.id) {
            branches.clear()
            branches.addAll(result)
        }
    }

    private val checkoutModes = mutableStateMapOf<String, String>()

    /** The checkout mode a new chat in this project starts with. */
    fun checkoutMode(projectId: String?): CheckoutMode =
        CheckoutMode.default(projectId, checkoutModes)

    fun rememberCheckoutMode(projectId: String?, mode: CheckoutMode) {
        if (projectId == null || mode !in CheckoutMode.available(projectId)) return
        if (checkoutModes[projectId] == mode.name) return
        checkoutModes[projectId] = mode.name
        val snapshot = checkoutModes.toMap()
        action { vault.write("new-chat-modes", WireJson.encodeToString(snapshot)) }
    }

    fun createChat(
        model: ModelChoice,
        branch: Branch?,
        fresh: Boolean,
        initialPrompt: String = "",
        done: () -> Unit,
    ) {
        if (busy) return
        createError = null
        viewModelScope.launch {
            busy = true
            try {
                createChatNow(model, branch, fresh, initialPrompt, done)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                createError =
                    if (RequestOutcome.uncertain(failure))
                        "The connection dropped while creating the chat. Try again; it will not be created twice."
                    else failure.message ?: "Could not create the chat. Try again."
            } finally {
                busy = false
            }
        }
    }

    private suspend fun createChatNow(
        model: ModelChoice,
        branch: Branch?,
        fresh: Boolean,
        initialPrompt: String,
        done: () -> Unit,
    ) {
        val project = selectedProject ?: throw ClientException("Choose a connected project.")
        val client = rpc ?: throw ClientException("Connecting to your computer. Try again in a moment.")
        val epoch = generation
        val archiveIdentity = identity()
        val isolated = fresh || (branch != null && !branch.current)
        // Refuse before recording an intent: a request that is never sent is not uncertain.
        require(!isolated || project.id != CanonicalWorkspace.PROJECTLESS) {
            "Choose a project to use a worktree or another branch."
        }
        // Branch selection is a server-side checkout; surface errors before creating a chat.
        if (!client.canonical && branch != null && !branch.current && !fresh)
            client.call(
                "git.switchBranch",
                obj(
                    "folderId" to project.id.json(),
                    "branch" to branch.name.json(),
                    "remote" to branch.remote?.json(),
                ),
            )
        val intentKey = "chat-create:$archiveIdentity:${project.id}"
        val wanted =
            obj(
                "projectId" to project.id.json(),
                "providerId" to model.provider.json(),
                "model" to model.id.json(),
                "runtimeMode" to "approval-required".json(),
                "workspacePolicy" to obj("_tag" to (if (fresh) "fresh" else "main").json()),
            )
        // An earlier attempt may have reached the host before the connection dropped. When this
        // host can tell, a request that never landed is dropped; one that did is opened again.
        val stored = vault.read(intentKey)?.let(WireJson::parseToJsonElement)
        var body = wanted
        if (stored != null && stored.field("body") != wanted) {
            val landed =
                !client.canonical ||
                    CanonicalWorkspace(client).exists(requireNotNull(stored.text("id")))
            if (landed) body = stored.field("body").jsonObject else vault.write(intentKey, null)
        }
        val recovered = body != wanted
        val (request, savedPrompt) = vault.prepareChatIntent(intentKey, body, initialPrompt)
        val response =
            try {
                if (client.canonical)
                    obj(
                        "initialSession" to
                            WireJson.encodeToJsonElement(
                                CanonicalWorkspace(client)
                                    .create(
                                        request,
                                        branch.takeUnless { recovered },
                                        request.field("workspacePolicy").text("_tag") == "fresh",
                                    )
                            )
                    )
                else client.call("chat.create", request)
            } catch (failure: Throwable) {
                if (!RequestOutcome.uncertain(failure)) vault.write(intentKey, null)
                throw failure
            }
        val created: Session = response.field("initialSession").decode()
        if (savedPrompt.isNotBlank() || initialPrompt != savedPrompt) {
            val operation = requireNotNull(request.text("operationId"))
            val item =
                PendingMessage(
                    operation,
                    UUID.nameUUIDFromBytes(("initial:" + operation).toByteArray()).toString(),
                    created.id,
                    savedPrompt.trim(),
                )
            vault.updateMetadata("$archiveIdentity|${created.id}") { record ->
                var updated =
                    if (savedPrompt.isBlank() || record.pending.any { it.id == item.id }) record
                    else record.copy(pending = record.pending + item)
                if (initialPrompt != savedPrompt)
                    updated =
                        updated.copy(
                            draftRevision = updated.draftRevision + 1,
                            draft = initialPrompt,
                        )
                updated
            }
        }
        vault.write(intentKey, null)
        if (epoch == generation) {
            newChatPrompt = ""
            focusComposer = true
            newChatOpen = false
            selectSession(created)
            done()
            loadSessions()
            if (recovered || initialPrompt != savedPrompt)
                error =
                    if (initialPrompt != savedPrompt)
                        "Opened the chat from your earlier attempt. Your new message is in the composer."
                    else "Opened the chat from your earlier attempt."
        } else {
            done()
            error = "Chat created on the original computer. Open it there to continue."
        }
    }

    fun permission(id: String, allow: Boolean) = action {
        rpc?.call(
            "permission.decide",
            obj(
                "requestId" to id.json(),
                "decision" to obj("_tag" to (if (allow) "AllowOnce" else "Deny").json()),
            ),
        )
    }

    fun answer(id: String, answers: JsonArray) = action {
        val session = selectedSession ?: return@action
        rpc?.takeIf { it.canonical }
            ?.let { client ->
                val interaction =
                    interactions.firstOrNull { it.text("id") == id }
                        ?: throw ClientException("This question is no longer pending.")
                CanonicalWorkspace(client)
                    .answer(session.id, id, interaction.array("questions"), answers)
                return@action
            }
        rpc?.call(
            "session.answerQuestion",
            obj(
                "sessionId" to session.id.json(),
                "commandId" to uuid().json(),
                "itemId" to id.json(),
                "answers" to answers,
            ),
        )
    }

    fun plan(id: String, outcome: String) = action {
        val session = selectedSession ?: return@action
        rpc?.call(
            "session.plan.respond",
            obj(
                "sessionId" to session.id.json(),
                "commandId" to uuid().json(),
                "toolCallId" to id.json(),
                "outcome" to outcome.json(),
            ),
        )
    }

    fun loadDiff(scope: String) {
        diffJob?.cancel()
        patches.clear()
        val session = selectedSession ?: return
        val client = rpc ?: return
        val current = sessionGeneration
        diffJob = action {
            diffLoading = true
            try {
                if (client.canonical) {
                    val snapshot = CanonicalWorkspace(client).diff(session.id, scope)
                    val loaded =
                        withContext(Dispatchers.Default) {
                            snapshot.array("files").map { file ->
                                val path = requireNotNull(file.text("path"))
                                val block =
                                    GitPatch.file(
                                        snapshot.text("patch").orEmpty(),
                                        path,
                                        file.text("oldPath"),
                                    )
                                Patch(
                                    path,
                                    DiffLine.parse(block.orEmpty()),
                                    scope,
                                    snapshot.flag("truncated"),
                                    if (block == null && !file.flag("binary"))
                                        "Patch unavailable for this file."
                                    else if (file.flag("binary")) "Binary file" else null,
                                )
                            }
                        }
                    if (current == sessionGeneration) patches.addAll(loaded)
                    return@action
                }

                client
                    .stream(
                        "git.reviewPatches",
                        obj(
                            "folderId" to session.projectId.json(),
                            "scope" to scope.json(),
                            "worktreeId" to session.worktreeId?.json(),
                        ),
                    )
                    .collect { frame ->
                        val result = frame.field("result")
                        val lines =
                            withContext(Dispatchers.Default) {
                                DiffLine.parse(result.text("patch").orEmpty())
                            }
                        if (current == sessionGeneration)
                            patches.add(
                                Patch(
                                    frame.text("path") ?: "Unknown file",
                                    lines,
                                    result.text("mode").orEmpty(),
                                    result.flag("truncated"),
                                    frame.text("error"),
                                )
                            )
                    }
            } finally {
                diffLoading = false
            }
        }
    }

    fun closeDiff() {
        diffJob?.cancel()
    }

    fun openAttachment(reference: JsonElement, sessionId: String) = action {
        require(selectedSession?.id == sessionId) { "Open this chat to view its attachments." }
        val client = rpc ?: throw ClientException("Reconnect to view this attachment.")
        var result =
            if (client.canonical) reference
            else
                client.call(
                    "attachments.read",
                    obj("id" to reference.field("id"), "sessionId" to sessionId.json()),
                ) ?: throw ClientException("Reconnect to view this attachment.")
        val encoded =
            if (client.canonical) ""
            else result.text("bytes") ?: throw ClientException("Missing attachment data.")
        require(encoded.length <= 100 * 1024 * 1024 * 4 / 3 + 8) {
            "Attachment exceeds the supported size."
        }
        val file =
            withContext(Dispatchers.IO) {
                val directory = java.io.File(app.cacheDir, "previews").apply { mkdirs() }
                directory
                    .listFiles()
                    ?.filter { System.currentTimeMillis() - it.lastModified() > 86_400_000 }
                    ?.forEach { it.delete() }
                val name =
                    (reference.text("originalName") ?: "Attachment")
                        .substringAfterLast('/')
                        .substringAfterLast('\\')
                        .takeLast(100)
                java.io.File(directory, "${uuid()}-$name").also {
                    try {
                        if (client.canonical)
                            it.outputStream().use { output ->
                                result =
                                    CanonicalWorkspace(client)
                                        .download(
                                            reference.text("path")
                                                ?: throw ClientException(
                                                    "Missing host attachment path."
                                                ),
                                            output,
                                        )
                            }
                        else it.writeBytes(Base64.decode(encoded, Base64.DEFAULT))
                    } catch (failure: Throwable) {
                        it.delete()
                        throw failure
                    }
                }
            }
        val uri =
            androidx.core.content.FileProvider.getUriForFile(
                app,
                "${BuildConfig.APPLICATION_ID}.files",
                file,
            )
        val intent =
            android.content
                .Intent(android.content.Intent.ACTION_VIEW)
                .setDataAndType(
                    uri,
                    result.text("mimeType")
                        ?: reference.text("mimeType")
                        ?: "application/octet-stream",
                )
                .addFlags(
                    android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION or
                        android.content.Intent.FLAG_ACTIVITY_NEW_TASK
                )
        try {
            app.startActivity(intent)
        } catch (_: android.content.ActivityNotFoundException) {
            throw ClientException("No installed app can open this file type.")
        }
    }

    fun removeAttachment(ref: JsonElement) = action {
        attachments.remove(ref)
        val key = selectedSession?.let(::archiveKey) ?: return@action
        val refs = attachments.toList()
        vault.updateMetadata(key) { it.copy(attachments = refs) }
    }

    fun attach(uri: Uri) = action {
        if (uploading || !synced) return@action
        val session = selectedSession ?: return@action
        val key = archiveKey(session)
        val current = sessionGeneration
        val client = rpc ?: return@action
        uploading = true
        try {
            val ref =
                withContext(Dispatchers.IO) {
                    val resolver = app.contentResolver
                    val name =
                        resolver
                            .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                            ?.use { if (it.moveToFirst()) it.getString(0) else null }
                            ?: "Attachment"
                    val mime = resolver.getType(uri) ?: "application/octet-stream"
                    val cache = java.io.File.createTempFile("attachment", null, app.cacheDir)
                    try {
                        var total = 0L
                        resolver.openInputStream(uri)?.use { input ->
                            cache.outputStream().use { output ->
                                val buffer = ByteArray(64 * 1024)
                                while (true) {
                                    ensureActive()
                                    val count = input.read(buffer)
                                    if (count < 0) break
                                    total += count
                                    require(total <= 100L * 1024 * 1024) {
                                        "Attachments must be 100 MB or smaller."
                                    }
                                    output.write(buffer, 0, count)
                                }
                            }
                        } ?: throw ClientException("Could not open this file.")
                        if (client.canonical)
                            return@withContext CanonicalWorkspace(client).upload(cache, name, mime)
                        val uploadId = uuid()
                        val chunk = ByteArray(2 * 1024 * 1024)
                        var offset = 0L
                        var result: JsonElement = JsonNull
                        cache.inputStream().use { input ->
                            do {
                                ensureActive()
                                var size = 0
                                while (size < chunk.size) {
                                    val read = input.read(chunk, size, chunk.size - size)
                                    if (read < 0) break
                                    size += read
                                }
                                val base =
                                    obj(
                                        "sessionId" to session.id.json(),
                                        "mimeType" to mime.json(),
                                        "originalName" to name.json(),
                                        "bytes" to
                                            Base64.encodeToString(
                                                    chunk.copyOf(size),
                                                    Base64.NO_WRAP,
                                                )
                                                .json(),
                                    )
                                result =
                                    if (total <= chunk.size) client.call("attachments.upload", base)
                                    else
                                        client.call(
                                            "attachments.uploadChunk",
                                            JsonObject(
                                                base +
                                                    mapOf(
                                                        "uploadId" to uploadId.json(),
                                                        "offset" to offset.json(),
                                                        "totalBytes" to total.json(),
                                                    )
                                            ),
                                        )
                                offset += size
                                require((offset == total) == (result != JsonNull)) {
                                    "The file upload did not complete."
                                }
                            } while (offset < total)
                        }
                        require(result.text("id") != null)
                        obj(
                            "id" to result.field("id"),
                            "mimeType" to mime.json(),
                            "originalName" to name.json(),
                        )
                    } finally {
                        cache.delete()
                    }
                }
            vault.updateMetadata(key) { it.copy(attachments = it.attachments + ref) }
            if (current == sessionGeneration) attachments.add(ref)
        } finally {
            if (current == sessionGeneration) uploading = false
        }
    }

    fun saveDevice(label: String, address: String, token: String, pin: String? = null) = action {
        Endpoint.validate(address)
        require(label.isNotBlank()) { "Give this device a name." }
        val next =
            SavedDevice(uuid(), label.trim(), address.trim(), token.trim(), certificatePin = pin)
        vault.write("devices", WireJson.encodeToString(devices.toList() + next))
        devices.add(next)
        connect(Source.Device(next))
    }

    fun forgetDevice(device: SavedDevice) = action {
        if ((source as? Source.Device)?.device?.id == device.id) disconnect()
        val remaining = devices.filterNot { it.id == device.id }
        vault.write("devices", WireJson.encodeToString(remaining))
        devices.remove(device)
    }

    fun pair(link: String, label: String, pin: String? = null, done: () -> Unit) = action {
        if (busy) return@action
        busy = true
        try {
            if (Endpoint.isNativeInvitation(link)) {
                require(pin == null) {
                    "Native invitations use the server's trusted TLS certificate."
                }
                val started =
                    withContext(Dispatchers.IO) {
                        NativePairing.request(
                            obj("op" to "start".json(), "link" to link.trim().json())
                        )
                    }
                val handle = requireNotNull(started.long("handle"))
                NativePairing.request(obj("op" to "close".json(), "handle" to handle.json()))
                val address = requireNotNull(started.text("url"))
                val verifier = RpcClient(address, pairingLink = link.trim())
                val identity = try {
                    verifier.connect()
                    requireNotNull(verifier.call("EngineInfo").text("deviceId"))
                } finally { verifier.close() }
                val origin = java.net.URI(address)
                val replaced = devices.filter { saved ->
                    saved.environmentId == identity || (saved.environmentId == null && saved.pairingLink != null &&
                        runCatching { java.net.URI(saved.address).let { it.host == origin.host && it.port == origin.port } }.getOrDefault(false))
                }
                val device =
                    SavedDevice(
                        replaced.firstOrNull()?.id ?: uuid(),
                        label.ifBlank { "Computer" },
                        address,
                        "",
                        environmentId = identity,
                        pairingLink = link.trim(),
                    )
                val updated = devices.filterNot { it in replaced } + device
                vault.write("devices", WireJson.encodeToString(updated))
                devices.clear()
                devices.addAll(updated)
                connect(Source.Device(device))
                done()
                return@action
            }

            val pairing = Endpoint.pairing(link)
            val deviceId = vault.read("device-id") ?: uuid().also { vault.write("device-id", it) }
            val result =
                withContext(Dispatchers.IO) {
                    val request =
                        Request.Builder()
                            .url(pairing.http + "/pair")
                            .post(
                                obj(
                                        "code" to pairing.code.json(),
                                        "deviceId" to deviceId.json(),
                                        "deviceLabel" to "Zuse Android".json(),
                                    )
                                    .toString()
                                    .toRequestBody("application/json".toMediaType())
                            )
                            .build()
                    pinnedHttp(pin)
                        .newBuilder()
                        .callTimeout(15, java.util.concurrent.TimeUnit.SECONDS)
                        .readTimeout(15, java.util.concurrent.TimeUnit.SECONDS)
                        .build()
                        .newCall(request)
                        .execute()
                        .use { response ->
                            require(response.isSuccessful) {
                                if (response.code == 410)
                                    "Pairing code expired. Generate a new code."
                                else "Pairing failed. Check the code and address."
                            }
                            WireJson.parseToJsonElement(response.body?.string().orEmpty())
                        }
                }
            val token = result.text("token") ?: throw ClientException("Missing pairing token.")
            require(token.startsWith("zt_"))
            val device =
                SavedDevice(
                    uuid(),
                    label.ifBlank { "Computer" },
                    pairing.websocket,
                    token,
                    result.text("environmentId"),
                    pin,
                )
            vault.write("devices", WireJson.encodeToString(devices.toList() + device))
            devices.add(device)
            connect(Source.Device(device))
            pairingLink = null
            done()
        } finally {
            busy = false
        }
    }

    companion object {
        const val CLOUD_PLACE = "cloud"
    }

    override fun onCleared() {
        rpc?.close()
        super.onCleared()
    }
}
