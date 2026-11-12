package sh.zuse.android

import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.*
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import sh.zuse.core.*

/**
 * A new chat in the chat screen itself. Nothing is created until the first message is sent; then
 * this screen becomes that chat. Where (only with several computers), project and mode are labelled
 * rows above the composer, and their defaults always make a request the host accepts.
 */
@Composable
fun NewChatPane(
    workspace: WorkspaceViewModel,
    showBack: Boolean,
    onBack: () -> Unit,
    onCloud: () -> Unit,
) {
    var model by remember { mutableStateOf<ModelChoice?>(null) }
    var branch by remember { mutableStateOf<Branch?>(null) }
    val projectId = workspace.selectedProject?.id
    // Without a project only working locally is possible; a project starts from its last choice.
    var mode by remember(projectId) { mutableStateOf(workspace.checkoutMode(projectId)) }
    val modes = CheckoutMode.available(projectId)
    var modelMenu by remember { mutableStateOf(false) }
    val preview = BuildConfig.DEBUG && workspace.selectedSession?.id == "preview-session"
    val places = workspace.places
    val input = remember { FocusRequester() }
    LaunchedEffect(Unit) {
        // Open straight into writing, with the keyboard up.
        withFrameNanos {}
        runCatching { input.requestFocus() }
    }
    LaunchedEffect(projectId) {
        workspace.loadBranches()
        branch = null
    }
    LaunchedEffect(workspace.models.toList()) {
        if (model !in workspace.models) model = workspace.models.firstOrNull()
    }
    val ready = (workspace.connected || preview) && workspace.selectedProject != null
    val prompt = workspace.newChatPrompt
    Column(Modifier.fillMaxHeight().widthIn(max = 760.dp).fillMaxWidth()) {
        PaneHeader(
            "New chat",
            leading = {
                if (showBack)
                    SmallIcon(Icons.AutoMirrored.Outlined.ArrowBack, "Back to chats", onBack)
            },
        )
        if (workspace.reconnecting)
            QuietBanner(
                "Reconnecting to ${workspace.sourceLabel}…",
                progress = true,
                action = "Retry",
                onAction = workspace::reconnect,
            )
        Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Text(
                "What are we building?",
                style = MaterialTheme.typography.headlineSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        // Where (only with several computers), project and mode sit right above the composer.
        Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp)) {
            if (places.size > 1)
                SelectorRow(
                    Icons.Outlined.Computer,
                    "Where",
                    workspace.sourceLabel,
                    places,
                    selected = workspace.currentPlace,
                    enabled = !workspace.busy,
                ) { key ->
                    if (key == WorkspaceViewModel.CLOUD_PLACE) onCloud()
                    else workspace.targetFor(key)?.let(workspace::connect)
                }
            SelectorRow(
                Icons.Outlined.FolderOpen,
                "Project",
                workspace.selectedProject?.name
                    ?: if (workspace.connected || preview) "Choose a project" else "Connecting…",
                workspace.projects.map { it.id to it.name },
                selected = projectId,
                enabled = !workspace.busy && workspace.projects.isNotEmpty(),
            ) { id ->
                workspace.projects.firstOrNull { it.id == id }?.let(workspace::selectProject)
            }
            if (modes.size > 1)
                SelectorRow(
                    Icons.Outlined.AccountTree,
                    "Mode",
                    mode.label,
                    modes.map { it.name to it.label },
                    selected = mode.name,
                    enabled = !workspace.busy,
                ) { name ->
                    mode = CheckoutMode.valueOf(name)
                    branch = null
                    workspace.rememberCheckoutMode(projectId, mode)
                }
            if (mode == CheckoutMode.Branch && workspace.branches.isNotEmpty())
                SelectorRow(
                    Icons.Outlined.CallSplit,
                    "Branch",
                    branch?.name
                        ?: workspace.branches.firstOrNull { it.current }?.name
                        ?: "Current branch",
                    workspace.branches.map { it.name to it.name },
                    selected = branch?.name,
                    enabled = !workspace.busy,
                ) { name ->
                    branch = workspace.branches.firstOrNull { it.name == name }
                }
            if (mode == CheckoutMode.Worktree)
                Text(
                    "Changes stay in a separate worktree.",
                    Modifier.padding(start = 40.dp, bottom = 4.dp),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
        }
        workspace.createError?.let {
            Text(
                it,
                Modifier.padding(horizontal = 24.dp, vertical = 4.dp).semantics {
                    liveRegion = LiveRegionMode.Polite
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }
        Box(
            Modifier.fillMaxWidth()
                .imePadding()
                .padding(start = 12.dp, end = 12.dp, top = 8.dp, bottom = 12.dp)
        ) {
            ComposerFrame(
                prompt,
                { workspace.newChatPrompt = it },
                expanded = true,
                enabled = !workspace.busy,
                inputDescription = "New chat message",
                focusRequester = input,
                leadingAction = {
                    Text(
                        when {
                            workspace.busy -> "Creating…"
                            workspace.source != null && !workspace.connected -> "Connecting…"
                            else -> ""
                        },
                        Modifier.padding(start = 14.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                },
                trailingAction = {
                    ModelTrigger(model?.name.orEmpty(), !workspace.busy) { modelMenu = true }
                    ComposerSend(
                        model != null && ready && prompt.isNotBlank() && !workspace.busy,
                        label = "Send message",
                    ) {
                        model?.let {
                            workspace.createChat(
                                it,
                                if (mode == CheckoutMode.Branch) branch else null,
                                mode == CheckoutMode.Worktree,
                                prompt,
                            ) {}
                        }
                    }
                },
            )
        }
    }
    if (modelMenu)
        ModelPickerSheet(workspace.models, model, { modelMenu = false }) {
            model = it
            modelMenu = false
        }
}

/** A labelled picker row: icon, label, value and an up/down affordance that opens a menu. */
@Composable
private fun SelectorRow(
    icon: ImageVector,
    label: String,
    value: String,
    options: List<Pair<String, String>>,
    selected: String?,
    enabled: Boolean = true,
    onSelect: (String) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier.fillMaxWidth()
                .height(48.dp)
                .clip(RoundedCornerShape(12.dp))
                .clickable(
                    enabled = enabled,
                    onClickLabel = "Change ${label.lowercase()}",
                    role = Role.DropdownList,
                ) {
                    open = true
                }
                .semantics(mergeDescendants = true) {}
                .padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Icon(icon, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(
                label,
                Modifier.width(72.dp),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                value,
                Modifier.weight(1f),
                style = MaterialTheme.typography.bodyMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Icon(
                Icons.Outlined.UnfoldMore,
                null,
                Modifier.size(14.dp),
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        DropdownMenu(open, { open = false }) {
            options.forEach { (key, name) ->
                DropdownMenuItem(
                    text = { Text(name) },
                    trailingIcon = {
                        if (key == selected) Icon(Icons.Outlined.Check, "Selected", Modifier.size(18.dp))
                    },
                    onClick = {
                        open = false
                        onSelect(key)
                    },
                )
            }
        }
    }
}
