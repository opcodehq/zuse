@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package sh.zuse.android

import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.*
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zuse.core.*

@Composable
fun Composer(workspace: WorkspaceViewModel) {
    var focused by remember { mutableStateOf(false) }
    var modelMenu by remember { mutableStateOf(false) }
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    BackHandler(focused) {
        focus.clearFocus()
        keyboard?.hide()
    }
    val expanded =
        focused ||
            modelMenu ||
            workspace.draft.isNotEmpty() ||
            workspace.attachments.isNotEmpty() ||
            workspace.uploading
    val inset by
        animateDpAsState(if (expanded) 12.dp else 24.dp, tween(180), label = "composer inset")
    val picker =
        rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
            uri?.let { workspace.attach(it) }
        }
    val running = workspace.activeTurnId != null
    // Mid-turn a written message is a follow-up, and the host queues it.
    val hasDraft = workspace.draft.isNotBlank() || workspace.attachments.isNotEmpty()
    val stopping = running && !hasDraft
    val placeholder =
        when {
            workspace.source != null && !workspace.connected -> "Reconnecting to your computer…"
            running -> "Send a follow-up…"
            else -> "Ask Zuse"
        }
    val sendEnabled =
        if (stopping) workspace.connected
        else
            workspace.inputsReady &&
                !workspace.sending &&
                !workspace.uploading &&
                (workspace.draft.isNotBlank() || workspace.attachments.isNotEmpty())
    val selectedModel = workspace.models.firstOrNull { it.id == workspace.selectedSession?.model }
    Box(
        Modifier.fillMaxWidth()
            .imePadding()
            .padding(start = inset, end = inset, top = 8.dp, bottom = 12.dp)
    ) {
        ComposerFrame(
            workspace.draft,
            workspace::updateDraft,
            expanded,
            workspace.inputsReady,
            placeholder = placeholder,
            onFocus = { focused = it },
            topContent = {
                val context = workspace.composerContext
                if (expanded && context != null)
                    Text(
                        context,
                        Modifier.padding(start = 16.dp, end = 16.dp, top = 10.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                if (workspace.attachments.isNotEmpty())
                    Row(
                        Modifier.padding(12.dp).horizontalScroll(rememberScrollState()),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        workspace.attachments.forEach { ref ->
                            Row(
                                Modifier.clip(RoundedCornerShape(12.dp))
                                    .background(MaterialTheme.colorScheme.secondaryContainer)
                                    .clickable { workspace.removeAttachment(ref) }
                                    .height(28.dp)
                                    .padding(horizontal = 10.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(6.dp),
                            ) {
                                Text(
                                    ref.text("originalName") ?: "Attachment",
                                    style = MaterialTheme.typography.labelMedium,
                                    maxLines = 1,
                                )
                                Icon(
                                    Icons.Outlined.Close,
                                    "Remove attachment",
                                    Modifier.size(12.dp),
                                )
                            }
                        }
                    }
            },
            leadingAction = {
                SmallIcon(
                    Icons.Outlined.Add,
                    "Add attachment",
                    { picker.launch(arrayOf("*/*")) },
                    workspace.synced && !workspace.uploading,
                )
                if (workspace.uploading)
                    CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
            },
            trailingAction = {
                if (expanded)
                    ModelTrigger(
                        selectedModel?.name ?: workspace.selectedSession?.model.orEmpty(),
                        workspace.synced && !stopping,
                    ) {
                        modelMenu = true
                    }
                ComposerSend(sendEnabled, stopping, !expanded) {
                    if (stopping) workspace.stop() else workspace.send()
                }
            },
        )
    }
    if (modelMenu)
        ModelPickerSheet(workspace.models, selectedModel, { modelMenu = false }) {
            workspace.changeModel(it)
            modelMenu = false
        }
}

/** Shared chat/new-chat chrome; the input keeps its node, selection and IME across size changes. */
@Composable
fun ComposerFrame(
    value: String,
    onChange: (String) -> Unit,
    expanded: Boolean,
    enabled: Boolean = true,
    inputDescription: String = "Ask Zuse",
    placeholder: String = "Ask Zuse",
    onFocus: (Boolean) -> Unit = {},
    topContent: @Composable () -> Unit = {},
    leadingAction: @Composable RowScope.() -> Unit,
    trailingAction: @Composable RowScope.() -> Unit,
) {
    // A pill at rest; it opens into a card, and the field keeps its node throughout.
    val radius by animateDpAsState(if (expanded) 18.dp else 22.dp, tween(180), label = "radius")
    val shape = RoundedCornerShape(radius)
    Surface(
        shape = shape,
        color = MaterialTheme.colorScheme.surfaceContainer,
        modifier =
            Modifier.fillMaxWidth()
                .border(.5.dp, MaterialTheme.colorScheme.outlineVariant, shape)
                .animateContentSize(tween(180))
                .semantics {
                    contentDescription = "Message composer"
                    stateDescription = if (expanded) "Expanded" else "Collapsed"
                },
    ) {
        Column {
            topContent()
            Box(Modifier.fillMaxWidth()) {
                val style =
                    MaterialTheme.typography.bodyLarge.copy(
                        fontSize = if (expanded) 17.sp else 16.sp,
                        lineHeight = if (expanded) 24.sp else 22.sp,
                        color = MaterialTheme.colorScheme.onSurface,
                    )
                BasicTextField(
                    value,
                    onChange,
                    enabled = enabled,
                    cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                    textStyle = style,
                    maxLines = 6,
                    modifier =
                        Modifier.fillMaxWidth()
                            .onFocusChanged { onFocus(it.isFocused) }
                            .then(
                                if (expanded) Modifier.heightIn(min = 96.dp, max = 196.dp)
                                else Modifier.heightIn(min = 44.dp)
                            )
                            .padding(
                                start = if (expanded) 16.dp else 46.dp,
                                end = if (expanded) 16.dp else 48.dp,
                                top = if (expanded) 12.dp else 0.dp,
                                bottom = if (expanded) 52.dp else 0.dp,
                            )
                            .semantics { contentDescription = inputDescription },
                    decorationBox = { inner ->
                        Box(
                            Modifier.fillMaxWidth(),
                            contentAlignment =
                                if (expanded) Alignment.TopStart else Alignment.CenterStart,
                        ) {
                            if (value.isEmpty())
                                Text(
                                    placeholder,
                                    style = style,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            inner()
                        }
                    },
                )
                Row(
                    Modifier.align(Alignment.BottomCenter)
                        .fillMaxWidth()
                        .height(44.dp)
                        .padding(horizontal = 2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    leadingAction()
                    Spacer(Modifier.weight(1f))
                    trailingAction()
                }
            }
        }
    }
}

@Composable
fun ComposerSend(
    enabled: Boolean,
    stopping: Boolean = false,
    round: Boolean = false,
    label: String = if (stopping) "Stop response" else "Send message",
    onClick: () -> Unit,
) {
    IconButton(onClick, enabled = enabled, modifier = Modifier.size(44.dp)) {
        Box(
            Modifier.size(30.dp)
                .clip(RoundedCornerShape(if (round) 15.dp else 8.dp))
                .background(
                    if (stopping) MaterialTheme.colorScheme.surfaceContainerHigh
                    else brandFill.copy(alpha = if (enabled) 1f else .45f)
                ),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                if (stopping) Icons.Outlined.Stop else Icons.Outlined.ArrowUpward,
                label,
                Modifier.size(if (stopping) 12.dp else 14.dp),
                tint = if (stopping) MaterialTheme.colorScheme.onSurface else Color.White,
            )
        }
    }
}

@Composable
fun ModelTrigger(name: String, enabled: Boolean = true, onClick: () -> Unit) {
    TextButton(
        onClick,
        enabled = enabled,
        modifier = Modifier.height(28.dp),
        contentPadding = PaddingValues(horizontal = 8.dp),
    ) {
        Text(
            name.ifEmpty { "Choose model" },
            style = MaterialTheme.typography.labelMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.widthIn(max = 140.dp),
        )
    }
}

@Composable
fun ModelPickerSheet(
    models: List<ModelChoice>,
    selected: ModelChoice?,
    onClose: () -> Unit,
    onSelect: (ModelChoice) -> Unit,
) {
    var search by rememberSaveable { mutableStateOf("") }
    ModalBottomSheet(
        onDismissRequest = onClose,
        sheetMaxWidth = 560.dp,
        dragHandle = { SheetHandle() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surface,
    ) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(.68f).padding(horizontal = 16.dp)) {
            PaneHeader(
                "Model",
                actions = { SmallIcon(Icons.Outlined.Close, "Close models", onClose) },
            )
            if (models.size > 8) CompactField("Search models", search, { search = it })
            LazyColumn {
                items(
                    models.filter {
                        search.isBlank() ||
                            it.name.contains(search, true) ||
                            it.provider.contains(search, true)
                    },
                    key = { "${it.provider}:${it.id}" },
                ) { model ->
                    Row(
                        Modifier.fillMaxWidth()
                            .clip(RoundedCornerShape(10.dp))
                            .clickable { onSelect(model) }
                            .heightIn(min = 48.dp)
                            .padding(horizontal = 12.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Icon(
                            Icons.Outlined.AutoAwesome,
                            null,
                            Modifier.size(17.dp),
                            tint = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Column(Modifier.weight(1f)) {
                            Text(model.name, style = MaterialTheme.typography.bodyMedium)
                            Text(
                                model.provider.replaceFirstChar(Char::uppercaseChar),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        if (model == selected)
                            Icon(
                                Icons.Outlined.Check,
                                null,
                                Modifier.size(17.dp),
                                tint = MaterialTheme.colorScheme.primary,
                            )
                    }
                }
            }
        }
    }
}
