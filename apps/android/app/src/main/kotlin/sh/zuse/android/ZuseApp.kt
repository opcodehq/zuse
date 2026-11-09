@file:OptIn(
    androidx.compose.material3.ExperimentalMaterial3Api::class,
    androidx.compose.material3.adaptive.ExperimentalMaterial3AdaptiveApi::class,
)

package sh.zuse.android

import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.*
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.foundation.shape.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.material3.adaptive.layout.*
import androidx.compose.material3.adaptive.navigation.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.*
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.*
import kotlinx.coroutines.launch
import sh.zuse.core.*

@Composable
fun ZuseApp(workspace: WorkspaceViewModel, openUri: (Uri) -> Unit) {
    val navigator = rememberListDetailPaneScaffoldNavigator<String>()
    val scope = rememberCoroutineScope()
    var settings by rememberSaveable { mutableStateOf(false) }
    var newChat by rememberSaveable { mutableStateOf(false) }
    val newChatState = rememberSaveableStateHolder()
    var cloud by rememberSaveable { mutableStateOf(false) }
    var diffs by rememberSaveable { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val error = workspace.error
    LaunchedEffect(error, settings, newChat, cloud, diffs) {
        if (
            error != null &&
                !settings &&
                !newChat &&
                !cloud &&
                !diffs &&
                workspace.pairingLink == null
        ) {
            snackbar.showSnackbar(error)
            if (workspace.error == error) workspace.error = null
        }
    }
    BackHandler(navigator.canNavigateBack()) { scope.launch { navigator.navigateBack() } }
    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        containerColor = MaterialTheme.colorScheme.background,
    ) { insets ->
        ListDetailPaneScaffold(
            modifier = Modifier.padding(insets).consumeWindowInsets(insets).fillMaxSize(),
            directive = navigator.scaffoldDirective.copy(defaultPanePreferredWidth = 320.dp),
            value = navigator.scaffoldValue,
            listPane = {
                AnimatedPane {
                    HomePane(
                        workspace,
                        onSettings = { settings = true },
                        onNewChat = {
                            if (
                                workspace.connected ||
                                    (BuildConfig.DEBUG &&
                                        workspace.selectedSession?.id == "preview-session")
                            )
                                newChat = true
                            else if (workspace.signedIn) cloud = true else settings = true
                        },
                        onOpen = { session ->
                            workspace.selectSession(session)
                            scope.launch {
                                navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, session.id)
                            }
                        },
                    )
                }
            },
            detailPane = {
                AnimatedPane {
                    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
                        key(workspace.conversationKey) {
                            ConversationPane(
                                workspace,
                                onBack = { scope.launch { navigator.navigateBack() } },
                                showBack =
                                    navigator.scaffoldValue[ListDetailPaneScaffoldRole.List] ==
                                        PaneAdaptedValue.Hidden,
                                onNewChat = {
                                    if (
                                        workspace.connected ||
                                            (BuildConfig.DEBUG &&
                                                workspace.selectedSession?.id == "preview-session")
                                    )
                                        newChat = true
                                    else if (workspace.signedIn) cloud = true else settings = true
                                },
                                onDiffs = { diffs = true },
                            )
                        }
                    }
                }
            },
        )
    }
    if (settings || workspace.pairingLink != null)
        SettingsSheet(
            workspace,
            openUri,
            onCloud = {
                settings = false
                cloud = true
            },
            onClose = {
                settings = false
                workspace.pairingLink = null
            },
        )
    if (newChat)
        newChatState.SaveableStateProvider("new-chat") {
            NewChatSheet(
                workspace,
                onClose = { newChat = false },
                onCloud = {
                    newChat = false
                    cloud = true
                },
                onCreated = {
                    newChat = false
                    newChatState.removeState("new-chat")
                    scope.launch {
                        navigator.navigateTo(
                            ListDetailPaneScaffoldRole.Detail,
                            workspace.selectedSession?.id,
                        )
                    }
                },
            )
        }
    if (cloud) CloudSheet(workspace, openUri, onClose = { cloud = false })
    if (diffs) DiffSheet(workspace) { diffs = false }
    LaunchedEffect(workspace.selectedSession?.id) {
        if (workspace.selectedSession != null)
            navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, workspace.selectedSession!!.id)
    }
}

@Composable
fun SmallIcon(icon: ImageVector, label: String, onClick: () -> Unit, enabled: Boolean = true) {
    IconButton(onClick = onClick, enabled = enabled, modifier = Modifier.size(44.dp)) {
        Icon(icon, label, Modifier.size(18.dp))
    }
}

@Composable
fun PaneHeader(
    title: String,
    subtitle: String? = null,
    leading: @Composable () -> Unit = {},
    actions: @Composable RowScope.() -> Unit = {},
) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        leading()
        Column(Modifier.weight(1f).padding(horizontal = 8.dp)) {
            Text(
                title,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (subtitle != null)
                Text(
                    subtitle,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                )
        }
        actions()
    }
}

@Composable
private fun HomePane(
    workspace: WorkspaceViewModel,
    onSettings: () -> Unit,
    onNewChat: () -> Unit,
    onOpen: (Session) -> Unit,
) {
    var search by rememberSaveable { mutableStateOf("") }
    LaunchedEffect(workspace.devices.toList(), workspace.computers.toList()) {
        workspace.refreshComputerInboxes()
    }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Row(
            Modifier.fillMaxWidth()
                .padding(start = 24.dp, end = 12.dp, top = 32.dp, bottom = 24.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text(
                    "Zuse",
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.primary,
                )
                Text(
                    "Chats",
                    style = MaterialTheme.typography.headlineLarge,
                    fontWeight = FontWeight.Bold,
                )
            }
            SmallIcon(Icons.Outlined.Settings, "Settings", onSettings)
            FilledIconButton(
                onClick = onNewChat,
                enabled = workspace.connected || workspace.signedIn,
            ) {
                Icon(Icons.Outlined.Add, "New chat")
            }
        }
        Surface(
            color = MaterialTheme.colorScheme.surfaceContainer,
            shape = RoundedCornerShape(12.dp),
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp).fillMaxWidth(),
        ) {
            Row(
                Modifier.height(56.dp).padding(horizontal = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    Icons.Outlined.Search,
                    null,
                    Modifier.size(22.dp),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                BasicSearch(search, { search = it }, Modifier.weight(1f))
            }
        }
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth(),
            contentPadding = PaddingValues(horizontal = 8.dp, vertical = 6.dp),
        ) {
            item {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        "Computers & chats",
                        Modifier.weight(1f),
                        style = MaterialTheme.typography.titleSmall,
                    )
                    SmallIcon(
                        Icons.Outlined.Refresh,
                        "Refresh computers",
                        workspace::refreshComputerInboxes,
                    )
                    SmallIcon(Icons.Outlined.Add, "Add computer", onSettings)
                }
            }
            workspace.computerInboxes.forEach { (key, inbox) ->
                val matches =
                    inbox.chats.filter {
                        search.isBlank() ||
                            it.title.contains(search, true) ||
                            inbox.label.contains(search, true)
                    }
                if (
                    search.isBlank() || matches.isNotEmpty() || inbox.label.contains(search, true)
                ) {
                    item(key = "computer:$key") {
                        ChatListRow(
                            inbox.label,
                            inbox.status,
                            workspace.source == inbox.target,
                            Icons.Outlined.Computer,
                        ) {
                            workspace.connect(inbox.target)
                        }
                    }
                    items(matches, key = { "host:$key:${it.id}" }, contentType = { "chat" }) {
                        session ->
                        Box(Modifier.padding(start = 20.dp)) {
                            ChatListRow(
                                session.title,
                                session.model,
                                workspace.source == inbox.target &&
                                    workspace.selectedSession?.id == session.id,
                                Icons.Outlined.ChatBubbleOutline,
                            ) {
                                workspace.openInboxChat(inbox.target, session)
                            }
                        }
                    }
                }
            }
            if (workspace.computerInboxes.values.none { it.target == workspace.source }) {
                items(
                    workspace.sessions.filter {
                        search.isBlank() || it.title.contains(search, true)
                    },
                    key = { "session:${it.id}" },
                    contentType = { "chat" },
                ) { session ->
                    ChatListRow(
                        session.title,
                        session.model,
                        workspace.selectedSession?.id == session.id,
                        Icons.Outlined.ChatBubbleOutline,
                    ) {
                        onOpen(session)
                    }
                }
            }
            val cloud =
                workspace.cloudChats.filter { search.isBlank() || it.title.contains(search, true) }
            if (cloud.isNotEmpty()) {
                item {
                    Text(
                        "Cloud chats",
                        Modifier.padding(start = 12.dp, top = 12.dp, bottom = 4.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                items(
                    cloud,
                    key = { "cloud:${it.workspaceId}:${it.chatId}" },
                    contentType = { "cloud" },
                ) { chat ->
                    ChatListRow(
                        chat.title,
                        chat.repositoryDisplayName,
                        (workspace.source as? Source.Cloud)?.chat?.chatId == chat.chatId,
                        Icons.Outlined.CloudQueue,
                    ) {
                        workspace.connect(Source.Cloud(chat))
                    }
                }
            }
            if (
                workspace.sessions.isEmpty() &&
                    cloud.isEmpty() &&
                    workspace.computerInboxes.isEmpty()
            )
                item {
                    Column(
                        Modifier.padding(horizontal = 16.dp, vertical = 24.dp)
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(12.dp))
                            .background(MaterialTheme.colorScheme.surfaceContainer)
                            .padding(horizontal = 24.dp, vertical = 32.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Icon(
                            Icons.Outlined.Forum,
                            null,
                            Modifier.size(24.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Text(
                            if (workspace.connected) "Start a chat" else "Connect your computer",
                            style = MaterialTheme.typography.headlineSmall,
                        )
                        Text(
                            if (workspace.connected) "Choose a project, then start a chat."
                            else
                                "Your chats and projects stay on your computer. Pair it to get started, or sign in for cloud chats.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                        )
                        CompactAction(
                            if (workspace.connected) "New chat" else "Get connected",
                            onClick = if (workspace.connected) onNewChat else onSettings,
                        )
                    }
                }
        }
    }
}

@Composable
private fun ChatListRow(
    title: String,
    detail: String,
    selected: Boolean,
    icon: ImageVector,
    onClick: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(
                if (selected) MaterialTheme.colorScheme.surfaceContainerHigh else Color.Transparent
            )
            .clickable(onClick = onClick)
            .heightIn(min = 46.dp)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(icon, null, Modifier.size(17.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                title,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                detail,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun BasicSearch(value: String, onChange: (String) -> Unit, modifier: Modifier = Modifier) {
    androidx.compose.foundation.text.BasicTextField(
        value,
        onChange,
        modifier.padding(10.dp),
        cursorBrush = androidx.compose.ui.graphics.SolidColor(MaterialTheme.colorScheme.primary),
        singleLine = true,
        textStyle =
            MaterialTheme.typography.bodyMedium.copy(color = MaterialTheme.colorScheme.onSurface),
        decorationBox = { inner ->
            if (value.isEmpty())
                Text(
                    "Search chats",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyMedium,
                )
            inner()
        },
    )
}

@Composable
private fun ConversationPane(
    workspace: WorkspaceViewModel,
    onBack: () -> Unit,
    showBack: Boolean,
    onNewChat: () -> Unit,
    onDiffs: () -> Unit,
) {
    val session = workspace.selectedSession
    var actions by remember { mutableStateOf(false) }
    var renaming by remember(session?.id) { mutableStateOf(false) }
    var archiving by remember(session?.id) { mutableStateOf(false) }
    var chatTitle by remember(session?.id) { mutableStateOf(session?.title.orEmpty()) }
    if (session != null && renaming)
        AlertDialog(
            onDismissRequest = { if (!workspace.chatActionBusy) renaming = false },
            title = { Text("Rename chat") },
            text = {
                OutlinedTextField(
                    chatTitle,
                    { chatTitle = it },
                    label = { Text("Chat title") },
                    singleLine = true,
                )
            },
            confirmButton = {
                TextButton(
                    enabled =
                        !workspace.chatActionBusy &&
                            chatTitle.isNotBlank() &&
                            chatTitle.length <= 500,
                    onClick = { workspace.renameChat(session, chatTitle) { renaming = false } },
                ) {
                    Text("Save name")
                }
            },
            dismissButton = {
                TextButton(enabled = !workspace.chatActionBusy, onClick = { renaming = false }) {
                    Text("Cancel")
                }
            },
        )
    if (session != null && archiving)
        AlertDialog(
            onDismissRequest = { if (!workspace.chatActionBusy) archiving = false },
            title = { Text("Archive chat?") },
            text = { Text("You can restore this chat from Archived chats in Settings.") },
            confirmButton = {
                TextButton(
                    enabled = !workspace.chatActionBusy,
                    onClick = {
                        workspace.setChatArchived(session, true) {
                            archiving = false
                            onBack()
                        }
                    },
                ) {
                    Text("Archive")
                }
            },
            dismissButton = {
                TextButton(enabled = !workspace.chatActionBusy, onClick = { archiving = false }) {
                    Text("Cancel")
                }
            },
        )
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    var follow by remember(session?.id) { mutableStateOf(true) }
    val atBottom by remember {
        derivedStateOf {
            list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ==
                list.layoutInfo.totalItemsCount - 1
        }
    }
    val dragging by list.interactionSource.collectIsDraggedAsState()
    LaunchedEffect(dragging, atBottom) { if (dragging) follow = atBottom }
    LaunchedEffect(session?.id) {
        if (workspace.messages.isNotEmpty())
            list.scrollToItem(
                workspace.messages.lastIndex + if (workspace.olderSequence != null) 1 else 0
            )
    }
    LaunchedEffect(
        workspace.messages.size,
        workspace.messages.lastOrNull(),
        workspace.pending.size,
    ) {
        withFrameNanos {}
        if (follow && list.layoutInfo.totalItemsCount > 0)
            list.scrollToItem(list.layoutInfo.totalItemsCount - 1)
    }
    Column(Modifier.fillMaxHeight().widthIn(max = 760.dp).fillMaxWidth()) {
        PaneHeader(
            session?.title ?: "Zuse",
            if (session != null)
                listOfNotNull(
                        workspace.selectedProject?.name,
                        if (BuildConfig.DEBUG && workspace.status.startsWith("Preview"))
                            "Sample data"
                        else null,
                    )
                    .joinToString(" · ")
            else null,
            leading = {
                if (showBack)
                    SmallIcon(Icons.AutoMirrored.Outlined.ArrowBack, "Back to chats", onBack)
            },
            actions = {
                Box {
                    SmallIcon(Icons.Outlined.MoreVert, "Chat actions", { actions = true })
                    DropdownMenu(actions, { actions = false }) {
                        if (session != null && workspace.canManageChats) {
                            DropdownMenuItem(
                                text = { Text("Rename chat") },
                                enabled = !workspace.chatActionBusy,
                                onClick = {
                                    actions = false
                                    chatTitle = session.title
                                    renaming = true
                                },
                            )
                            DropdownMenuItem(
                                text = { Text("Archive chat") },
                                enabled = !workspace.chatActionBusy,
                                onClick = {
                                    actions = false
                                    archiving = true
                                },
                            )
                        }
                        DropdownMenuItem(
                            text = { Text("New chat") },
                            enabled =
                                workspace.connected ||
                                    workspace.signedIn ||
                                    (BuildConfig.DEBUG && session?.id == "preview-session"),
                            onClick = {
                                actions = false
                                onNewChat()
                            },
                        )
                        if (session != null)
                            DropdownMenuItem(
                                text = { Text("Review changes") },
                                enabled = workspace.connected,
                                onClick = {
                                    actions = false
                                    onDiffs()
                                },
                            )
                        if (workspace.source != null)
                            DropdownMenuItem(
                                text = { Text("Reconnect") },
                                onClick = {
                                    actions = false
                                    workspace.reconnect()
                                },
                            )
                    }
                }
            },
        )
        if (session == null) {
            Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("What are we building?", style = MaterialTheme.typography.headlineSmall)
                    Text(
                        "Choose a chat, or start a new one.",
                        Modifier.padding(8.dp),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        } else {
            Box(Modifier.weight(1f)) {
                LazyColumn(
                    state = list,
                    modifier =
                        Modifier.fillMaxSize().semantics { contentDescription = "Chat transcript" },
                    contentPadding = PaddingValues(horizontal = 16.dp, vertical = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    if (workspace.olderSequence != null)
                        item(key = "older") {
                            TextButton(
                                onClick = { workspace.loadOlder() },
                                enabled = !workspace.paging,
                            ) {
                                Text(
                                    if (workspace.paging) "Loading history…"
                                    else "Load earlier messages"
                                )
                            }
                        }
                    items(workspace.messages, key = { it.id }, contentType = { it.kind }) { message
                        ->
                        MessageRow(
                            message,
                            onAttachment = { ref ->
                                workspace.openAttachment(ref, message.sessionId)
                            },
                            responded = message.id in workspace.respondedUi,
                            onRespond =
                                if (workspace.source != null && workspace.inputsReady)
                                    { text -> workspace.respondToUi(message, text) }
                                else null,
                            uiDraft = workspace.uiDrafts[message.id].orEmpty(),
                            onUiDraftChange = { name, value ->
                                workspace.updateUiDraft(message, name, value)
                            },
                        )
                    }
                    items(
                        workspace.pending,
                        key = { "pending:${it.id}" },
                        contentType = { "pending" },
                    ) { item ->
                        Surface(
                            color = MaterialTheme.colorScheme.surfaceContainerHigh,
                            shape = RoundedCornerShape(12.dp),
                        ) {
                            Column(Modifier.padding(12.dp)) {
                                Text(item.text, style = MaterialTheme.typography.bodyMedium)
                                Text(
                                    if (item.attempted)
                                        "Delivery unconfirmed. Check the transcript before retrying."
                                    else "Queued · waiting for synchronization",
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                                Row {
                                    if (item.attempted)
                                        TextButton(
                                            onClick = { workspace.flushOutbox(item.id) },
                                            enabled = workspace.synced && !workspace.sending,
                                        ) {
                                            Text("Retry same message")
                                        }
                                    TextButton(
                                        onClick = { workspace.removePending(item.id) },
                                        enabled = !workspace.sending,
                                    ) {
                                        Text("Remove")
                                    }
                                }
                            }
                        }
                    }
                    if (workspace.activeTurnId != null)
                        item("working") {
                            Text(
                                "Zuse is working…",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                }
                if (!atBottom && workspace.messages.isNotEmpty())
                    FilledTonalIconButton(
                        onClick = {
                            follow = true
                            scope.launch {
                                list.animateScrollToItem(list.layoutInfo.totalItemsCount - 1)
                            }
                        },
                        modifier = Modifier.align(Alignment.BottomEnd).padding(12.dp),
                    ) {
                        Icon(Icons.Outlined.ArrowDownward, "Jump to latest")
                    }
            }
            InteractionBar(workspace)
            Composer(workspace)
        }
    }
}
