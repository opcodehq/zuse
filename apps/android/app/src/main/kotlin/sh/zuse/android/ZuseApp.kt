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
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.*
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.*
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.launch
import sh.zuse.core.*

@Composable
fun ZuseApp(workspace: WorkspaceViewModel, openUri: (Uri) -> Unit) {
    val navigator = rememberListDetailPaneScaffoldNavigator<String>()
    val scope = rememberCoroutineScope()
    var settings by rememberSaveable { mutableStateOf(false) }
    var settingsPage by rememberSaveable { mutableStateOf("home") }
    var cloud by rememberSaveable { mutableStateOf(false) }
    var diffs by rememberSaveable { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val error = workspace.error
    LaunchedEffect(error, settings, cloud, diffs) {
        if (
            error != null &&
                !settings &&
                !cloud &&
                !diffs &&
                workspace.pairingLink == null
        ) {
            snackbar.showSnackbar(error)
            if (workspace.error == error) workspace.error = null
        }
    }
    val openSettings = { page: String ->
        settingsPage = page
        settings = true
    }
    val startNewChat = {
        val preview = BuildConfig.DEBUG && workspace.selectedSession?.id == "preview-session"
        workspace.prepareNewChat()
        when {
            (workspace.source != null && workspace.source !is Source.Cloud) || preview -> {
                workspace.openNewChat()
                scope.launch { navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, "new") }
            }
            workspace.signedIn -> cloud = true
            else -> openSettings("pair")
        }
        Unit
    }
    // Leaving an unsent new chat leaves nothing behind but its draft.
    val back: () -> Unit = {
        scope.launch {
            navigator.navigateBack()
            if (workspace.newChatOpen) workspace.closeNewChat()
        }
    }
    BackHandler(navigator.canNavigateBack()) { back() }
    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        containerColor = MaterialTheme.colorScheme.background,
    ) { insets ->
        ListDetailPaneScaffold(
            modifier = Modifier.padding(insets).consumeWindowInsets(insets).fillMaxSize(),
            directive = navigator.scaffoldDirective.copy(defaultPanePreferredWidth = 360.dp),
            value = navigator.scaffoldValue,
            listPane = {
                AnimatedPane {
                    HomePane(
                        workspace,
                        showSelection =
                            navigator.scaffoldValue[ListDetailPaneScaffoldRole.Detail] !=
                                PaneAdaptedValue.Hidden,
                        onSettings = { openSettings("home") },
                        onPair = { openSettings("pair") },
                        onSignIn = { workspace.signIn(openUri) },
                        onNewChat = startNewChat,
                        onOpen = { row ->
                            workspace.openChat(row)
                            scope.launch {
                                navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, row.chatId)
                            }
                        },
                    )
                }
            },
            detailPane = {
                AnimatedPane {
                    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.TopCenter) {
                        if (workspace.newChatOpen)
                            NewChatPane(
                                workspace,
                                showBack =
                                    navigator.scaffoldValue[ListDetailPaneScaffoldRole.List] ==
                                        PaneAdaptedValue.Hidden,
                                onBack = back,
                                onCloud = { cloud = true },
                            )
                        else key(workspace.conversationKey) {
                            ConversationPane(
                                workspace,
                                onBack = { scope.launch { navigator.navigateBack() } },
                                showBack =
                                    navigator.scaffoldValue[ListDetailPaneScaffoldRole.List] ==
                                        PaneAdaptedValue.Hidden,
                                onNewChat = startNewChat,
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
            initialPage = settingsPage,
            onCloud = {
                settings = false
                cloud = true
            },
            onClose = {
                settings = false
                settingsPage = "home"
                workspace.pairingLink = null
            },
        )
    if (cloud) CloudSheet(workspace, openUri, onClose = { cloud = false })
    if (diffs) DiffSheet(workspace) { diffs = false }
    LaunchedEffect(workspace.selectedSession?.id) {
        val id = workspace.selectedSession?.id
        if (id != null && id != workspace.restoredAtLaunch)
            navigator.navigateTo(ListDetailPaneScaffoldRole.Detail, id)
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
    showSelection: Boolean,
    onSettings: () -> Unit,
    onPair: () -> Unit,
    onSignIn: () -> Unit,
    onNewChat: () -> Unit,
    onOpen: (ChatRow) -> Unit,
) {
    var search by rememberSaveable { mutableStateOf("") }
    LaunchedEffect(workspace.devices.toList(), workspace.computers.toList(), workspace.signedIn) {
        // Returning to the list refreshes in place; only pulling shows the indicator.
        workspace.refreshComputerInboxes(quiet = true)
    }
    // Keep previews and run status fresh while the list is on screen.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    LaunchedEffect(lifecycle) {
        lifecycle.repeatOnLifecycle(Lifecycle.State.RESUMED) {
            while (true) {
                kotlinx.coroutines.delay(30_000)
                workspace.refreshComputerInboxes(quiet = true)
            }
        }
    }
    val places = workspace.places
    val filter = workspace.activePlaceFilter
    val groups = ChatList.groups(workspace.chatRows(), search, filter)
    val unconfigured = workspace.unconfigured
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Column(Modifier.fillMaxSize()) {
            Row(
                Modifier.fillMaxWidth()
                    .heightIn(min = 64.dp)
                    .padding(start = 20.dp, end = 8.dp, top = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    "Chats",
                    Modifier.weight(1f).semantics { heading() },
                    style = MaterialTheme.typography.headlineMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                if (places.size > 1) PlaceSwitcher(workspace, places, filter)
                SmallIcon(Icons.Outlined.Settings, "Settings", onSettings)
            }
            if (!unconfigured) {
                SearchField(search, { search = it })
                HomeBanner(workspace)
            }
            val refreshing = workspace.inboxRefreshing || workspace.cloudLoading
            PullToRefreshBox(
                refreshing,
                onRefresh = workspace::refreshHome,
                modifier = Modifier.weight(1f).fillMaxWidth(),
            ) {
                LazyColumn(
                    Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(top = 4.dp, bottom = 96.dp),
                ) {
                    groups.forEach { group ->
                        item(key = "group:${group.key}", contentType = "header") {
                            ProjectHeader(group.title, group.place)
                        }
                        items(group.rows, key = { it.key }, contentType = { "chat" }) { row ->
                            ChatListRow(row, showSelection && workspace.isOpen(row)) {
                                onOpen(row)
                            }
                        }
                    }
                    if (groups.isEmpty())
                        item(key = "empty") {
                            when {
                                unconfigured -> ConnectEmptyState(onPair, onSignIn)
                                search.isNotBlank() ->
                                    QuietEmptyState(
                                        "No matching chats",
                                        "Nothing matches “${search.trim()}”.",
                                    )
                                refreshing ||
                                    workspace.computerInboxes.values.any { it.reachable == null } ->
                                    Spacer(Modifier.height(1.dp))
                                workspace.unreachablePlaces.isNotEmpty() ->
                                    QuietEmptyState(
                                        "No chats to show",
                                        "Chats appear here once the computer is reachable.",
                                    )
                                else ->
                                    QuietEmptyState(
                                        "No chats yet",
                                        "Start a chat and it will show up here.",
                                    )
                            }
                        }
                }
            }
        }
        if (!unconfigured)
            ExtendedFloatingActionButton(
                onClick = onNewChat,
                containerColor = brandFill,
                contentColor = Color(0xFF17220B),
                elevation = FloatingActionButtonDefaults.elevation(2.dp, 2.dp, 2.dp, 2.dp),
                modifier = Modifier.align(Alignment.BottomEnd).padding(20.dp),
            ) {
                Icon(Icons.Outlined.Edit, null, Modifier.size(20.dp))
                Spacer(Modifier.width(12.dp))
                Text("New chat", style = MaterialTheme.typography.labelLarge)
            }
    }
}

/** Shown only with more than one place to run chats: every place, or one computer or Cloud. */
@Composable
private fun PlaceSwitcher(
    workspace: WorkspaceViewModel,
    places: List<Pair<String, String>>,
    filter: String?,
) {
    var open by remember { mutableStateOf(false) }
    val label = places.firstOrNull { it.first == filter }?.second ?: "All computers"
    Box {
        TextButton(
            onClick = { open = true },
            contentPadding = PaddingValues(start = 12.dp, end = 8.dp),
            colors =
                ButtonDefaults.textButtonColors(
                    contentColor = MaterialTheme.colorScheme.onSurfaceVariant
                ),
            modifier =
                Modifier.semantics { contentDescription = "Showing chats from $label" },
        ) {
            Text(
                label,
                Modifier.widthIn(max = 150.dp),
                style = MaterialTheme.typography.labelLarge,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Icon(Icons.Outlined.ExpandMore, null, Modifier.padding(start = 2.dp).size(18.dp))
        }
        DropdownMenu(open, { open = false }) {
            (listOf<Pair<String?, String>>(null to "All computers") + places).forEach {
                (key, name) ->
                DropdownMenuItem(
                    text = { Text(name) },
                    trailingIcon = {
                        if (key == filter) Icon(Icons.Outlined.Check, "Selected", Modifier.size(18.dp))
                    },
                    onClick = {
                        open = false
                        workspace.placeFilter = key
                    },
                )
            }
        }
    }
}

@Composable
private fun SearchField(value: String, onChange: (String) -> Unit) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        shape = RoundedCornerShape(24.dp),
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth(),
    ) {
        Row(
            Modifier.height(48.dp).padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                Icons.Outlined.Search,
                null,
                Modifier.size(20.dp),
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            BasicSearch(value, onChange, Modifier.weight(1f))
            if (value.isNotEmpty()) SmallIcon(Icons.Outlined.Close, "Clear search", { onChange("") })
        }
    }
}

/** Connection state only when it matters: a computer that cannot be reached right now. */
@Composable
private fun HomeBanner(workspace: WorkspaceViewModel) {
    val unreachable = workspace.unreachablePlaces
    if (unreachable.isEmpty()) return
    QuietBanner(
        if (unreachable.size == 1) "Can’t reach ${unreachable.first()}"
        else "Can’t reach ${unreachable.size} computers",
        icon = Icons.Outlined.CloudOff,
        action = "Retry",
        onAction = { workspace.refreshHome() },
    )
}

@Composable
fun QuietBanner(
    text: String,
    icon: ImageVector? = null,
    progress: Boolean = false,
    action: String? = null,
    onAction: () -> Unit = {},
) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        shape = RoundedCornerShape(12.dp),
        modifier =
            Modifier.padding(horizontal = 16.dp, vertical = 6.dp)
                .fillMaxWidth()
                .semantics { liveRegion = LiveRegionMode.Polite },
    ) {
        Row(
            Modifier.heightIn(min = 40.dp).padding(start = 14.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (progress)
                CircularProgressIndicator(
                    Modifier.size(14.dp),
                    strokeWidth = 2.dp,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            else if (icon != null)
                Icon(
                    icon,
                    null,
                    Modifier.size(16.dp),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            Text(
                text,
                Modifier.weight(1f).padding(vertical = 10.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (action != null)
                TextButton(onClick = onAction) {
                    Text(action, style = MaterialTheme.typography.labelLarge)
                }
        }
    }
}

@Composable
private fun ProjectHeader(title: String, place: String?) {
    Row(
        Modifier.fillMaxWidth()
            .padding(start = 20.dp, end = 20.dp, top = 20.dp, bottom = 6.dp)
            .semantics(mergeDescendants = true) { heading() },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            title,
            Modifier.weight(1f, fill = false),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (place != null)
            Text(
                " · $place",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = .7f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
    }
}

private val warning = Color(0xFFC98A1B)

@Composable
private fun ChatListRow(row: ChatRow, selected: Boolean, onClick: () -> Unit) {
    val time = ChatList.relativeTime(row.activityAt)
    val detail = ChatList.detail(row)
    val statusColor =
        when (row.status) {
            ChatStatus.NeedsInput -> warning
            ChatStatus.Failed -> MaterialTheme.colorScheme.error
            else -> MaterialTheme.colorScheme.onSurfaceVariant
        }
    Row(
        Modifier.fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(
                if (selected) MaterialTheme.colorScheme.surfaceContainerHigh else Color.Transparent
            )
            .clickable(onClickLabel = "Open chat", onClick = onClick)
            .semantics(mergeDescendants = true) {}
            .heightIn(min = 64.dp)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    row.title,
                    Modifier.weight(1f),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodyLarge,
                    fontWeight = FontWeight.Medium,
                )
                if (time.isNotEmpty())
                    Text(
                        time,
                        Modifier.padding(start = 12.dp),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
            }
            if (detail != null || row.branch != null)
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    when (row.status) {
                        ChatStatus.Working ->
                            CircularProgressIndicator(
                                Modifier.size(10.dp),
                                strokeWidth = 1.5.dp,
                                color = MaterialTheme.colorScheme.primary,
                            )
                        ChatStatus.NeedsInput,
                        ChatStatus.Failed -> Box(Modifier.size(7.dp).background(statusColor, CircleShape))
                        else -> {}
                    }
                    Text(
                        detail.orEmpty(),
                        Modifier.weight(1f),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        style = MaterialTheme.typography.bodyMedium,
                        color = statusColor,
                    )
                    row.branch?.let { branch ->
                        Icon(
                            Icons.Outlined.CallSplit,
                            null,
                            Modifier.size(12.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Text(
                            branch,
                            Modifier.widthIn(max = 120.dp).semantics {
                                contentDescription = "Branch $branch"
                            },
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.labelMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
        }
    }
}

@Composable
private fun ConnectEmptyState(onPair: () -> Unit, onSignIn: () -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 32.dp).padding(top = 72.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier.size(64.dp)
                .background(MaterialTheme.colorScheme.surfaceContainerHigh, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                Icons.Outlined.Laptop,
                null,
                Modifier.size(28.dp),
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Text(
            "Connect your computer",
            Modifier.padding(top = 24.dp),
            style = MaterialTheme.typography.headlineSmall,
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        )
        Text(
            "Your chats run on your computer. Open Zuse there, choose Devices, and scan the code or paste the pairing link.",
            Modifier.padding(top = 8.dp),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        )
        Spacer(Modifier.height(28.dp))
        CompactAction("Pair a computer", onClick = onPair)
        TextButton(onClick = onSignIn, modifier = Modifier.padding(top = 8.dp)) {
            Text("Sign in to Zuse Cloud")
        }
    }
}

@Composable
private fun QuietEmptyState(title: String, body: String) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 64.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(title, style = MaterialTheme.typography.titleMedium)
        Text(
            body,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        )
    }
}

@Composable
private fun BasicSearch(value: String, onChange: (String) -> Unit, modifier: Modifier = Modifier) {
    androidx.compose.foundation.text.BasicTextField(
        value,
        onChange,
        modifier.padding(horizontal = 12.dp, vertical = 10.dp).semantics {
            contentDescription = "Search chats"
        },
        cursorBrush = androidx.compose.ui.graphics.SolidColor(MaterialTheme.colorScheme.primary),
        singleLine = true,
        textStyle =
            MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
        decorationBox = { inner ->
            if (value.isEmpty())
                Text(
                    "Search",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodyLarge,
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
    val groups by remember { derivedStateOf { TranscriptGroup.project(workspace.messages.toList()) } }
    // A finished turn may have renamed the chat; refresh the list behind it quietly.
    LaunchedEffect(workspace.activeTurnId == null) {
        if (workspace.activeTurnId == null && session != null) {
            workspace.refreshComputerInboxes(quiet = true)
            // Hosts name a chat shortly after its first reply.
            kotlinx.coroutines.delay(6_000)
            workspace.refreshComputerInboxes(quiet = true)
        }
    }
    val dragging by list.interactionSource.collectIsDraggedAsState()
    LaunchedEffect(dragging, atBottom) { if (dragging) follow = atBottom }
    LaunchedEffect(session?.id) {
        if (workspace.messages.isNotEmpty())
            list.scrollToItem(
                groups.lastIndex + if (workspace.olderSequence != null) 1 else 0
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
                        workspace.composerContext,
                        session.branch,
                        if (BuildConfig.DEBUG && workspace.status.startsWith("Preview"))
                            "Sample data"
                        else null,
                    )
                    .joinToString(" · ")
                    .ifEmpty { null }
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
        if (workspace.reconnecting)
            QuietBanner(
                "Reconnecting to ${workspace.sourceLabel}…",
                progress = true,
                action = "Retry",
                onAction = workspace::reconnect,
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
                    items(
                        groups,
                        key = { it.id },
                        contentType = { if (it.isActivity) "activity" else it.rows[0].message.kind },
                    ) { group ->
                        if (group.isActivity) {
                            ToolActivityGroup(
                                group,
                                live =
                                    group.hasRunningTool ||
                                        (workspace.activeTurnId != null && group == groups.last()),
                            )
                            return@items
                        }
                        val message = group.rows[0].message
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
                        PendingRow(workspace, item)
                    }
                    if (workspace.activeTurnId != null)
                        item("working") {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                                modifier = Modifier.semantics(mergeDescendants = true) {},
                            ) {
                                CircularProgressIndicator(
                                    Modifier.size(12.dp),
                                    strokeWidth = 1.5.dp,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                                Text(
                                    "Working…",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
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

/** A message waiting on this phone: queued, refused (certainly not sent), or unconfirmed. */
@Composable
private fun PendingRow(workspace: WorkspaceViewModel, item: PendingMessage) {
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
        Surface(
            color = MaterialTheme.colorScheme.surfaceContainerHigh.copy(alpha = .6f),
            shape = RoundedCornerShape(16.dp, 4.dp, 16.dp, 16.dp),
            modifier = Modifier.widthIn(max = 520.dp),
        ) {
            Text(
                item.text,
                Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        val rejected = item.rejected
        Text(
            when {
                rejected != null -> "Not sent · $rejected"
                item.attempted -> "Delivery unconfirmed. Check the reply before sending again."
                else -> "Waiting to send"
            },
            Modifier.padding(top = 4.dp, end = 4.dp),
            style = MaterialTheme.typography.labelSmall,
            color =
                if (rejected != null) MaterialTheme.colorScheme.error
                else MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Row {
            if (rejected != null || item.attempted)
                TextButton(
                    onClick = { workspace.flushOutbox(item.id) },
                    enabled = workspace.synced && !workspace.sending,
                ) {
                    Text(if (rejected != null) "Try again" else "Retry same message")
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
