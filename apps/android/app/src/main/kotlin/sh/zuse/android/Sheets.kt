@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package sh.zuse.android

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.*
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.*
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.flow.catch
import sh.zuse.core.*

/** 28dp visible controls with compact tonal surfaces and a 44dp interactive row. */
@Composable
fun CompactField(
    label: String,
    value: String,
    onChange: (String) -> Unit,
    secret: Boolean = false,
    enabled: Boolean = true,
    placeholder: String? = null,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        placeholder = placeholder?.let { text -> { Text(text) } },
        enabled = enabled,
        singleLine = true,
        modifier =
            Modifier.fillMaxWidth().padding(vertical = 4.dp).semantics {
                contentDescription = label
            },
        shape = RoundedCornerShape(12.dp),
        keyboardOptions =
            androidx.compose.foundation.text.KeyboardOptions(
                keyboardType =
                    if (secret) androidx.compose.ui.text.input.KeyboardType.Password
                    else androidx.compose.ui.text.input.KeyboardType.Text,
                autoCorrectEnabled = false,
            ),
        visualTransformation =
            if (secret) PasswordVisualTransformation() else VisualTransformation.None,
    )
}

@Composable
fun CompactAction(label: String, enabled: Boolean = true, onClick: () -> Unit) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.heightIn(min = 48.dp),
        contentPadding = PaddingValues(horizontal = 24.dp, vertical = 12.dp),
        shape = RoundedCornerShape(12.dp),
        colors =
            ButtonDefaults.buttonColors(
                containerColor = brandFill,
                contentColor = Color(0xFF17220B),
            ),
    ) {
        Text(label, style = MaterialTheme.typography.labelLarge)
    }
}

@Composable
private fun SectionLabel(text: String) {
    Text(
        text,
        Modifier.padding(top = 4.dp, bottom = 0.dp),
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
fun SettingsSheet(
    workspace: WorkspaceViewModel,
    openUri: (Uri) -> Unit,
    onCloud: () -> Unit,
    onClose: () -> Unit,
    initialPage: String = "home",
) {
    var page by rememberSaveable {
        mutableStateOf(if (workspace.pairingLink != null) "pair" else initialPage)
    }
    LaunchedEffect(page, workspace.connected) {
        if (page == "archives" && workspace.canManageChats) workspace.loadArchivedChats()
    }
    var label by rememberSaveable { mutableStateOf("") }
    var address by rememberSaveable { mutableStateOf("") }
    var token by rememberSaveable { mutableStateOf("") }
    var pin by rememberSaveable { mutableStateOf("") }
    var pairing by rememberSaveable { mutableStateOf(workspace.pairingLink.orEmpty()) }
    var nearbySelection by remember { mutableStateOf<NearbyComputer?>(null) }
    var code by rememberSaveable { mutableStateOf("") }
    var advanced by rememberSaveable { mutableStateOf(false) }
    val scanner =
        rememberLauncherForActivityResult(ScanContract()) { result ->
            result.contents?.let { pairing = it }
        }
    val context = LocalContext.current
    var nearbyError by remember { mutableStateOf<String?>(null) }
    val nearby =
        if (page == "computers" || page == "pair") {
            val rows by
                remember(context) {
                        NearbyBrowser(context).computers.catch {
                            nearbyError = it.message
                            emit(emptyList())
                        }
                    }
                    .collectAsStateWithLifecycle(emptyList())
            rows
        } else emptyList()
    ModalBottomSheet(
        onDismissRequest = onClose,
        sheetMaxWidth = 560.dp,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surface,
        dragHandle = { SheetHandle() },
        properties =
            ModalBottomSheetProperties(shouldDismissOnBackPress = page == "home" || page == initialPage),
    ) {
        androidx.activity.compose.BackHandler(page != "home" && page != initialPage) {
            page = if (page in listOf("pair", "manual")) "computers" else "home"
        }
        Column(
            Modifier.fillMaxWidth()
                .heightIn(
                    max =
                        androidx.compose.ui.platform.LocalConfiguration.current.screenHeightDp.dp *
                            .85f
                )
        ) {
            PaneHeader(
                when (page) {
                    "home" -> "Settings"
                    "archives" -> "Archived chats"
                    "account" -> "Account"
                    "computers" -> "Computers"
                    "pair" -> "Pair a computer"
                    "manual" -> "Manual connection"
                    else -> "Help"
                },
                leading = {
                    if (page != "home")
                        SmallIcon(
                            Icons.AutoMirrored.Outlined.ArrowBack,
                            "Back to settings",
                            { page = if (page in listOf("pair", "manual")) "computers" else "home" },
                        )
                },
                actions = { SmallIcon(Icons.Outlined.Close, "Close settings", onClose) },
            )
            key(page) {
                Column(
                    Modifier.weight(1f, fill = false)
                        .verticalScroll(rememberScrollState())
                        .padding(horizontal = 16.dp)
                        .imePadding(),
                    verticalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    workspace.error?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                    when (page) {
                        "home" -> {
                            SettingsGroup {
                                SettingsRow(
                                    Icons.Outlined.AccountCircle,
                                    if (workspace.signedIn) "Account" else "Sign in",
                                    "Cloud chats and remote access",
                                ) {
                                    page = "account"
                                }
                            }
                            SettingsGroup {
                                SettingsRow(
                                    Icons.Outlined.Computer,
                                    "Computers",
                                    "${workspace.devices.size + workspace.computers.size} saved · local, remote & Tailscale",
                                ) {
                                    page = "computers"
                                }
                                SettingsRow(
                                    Icons.Outlined.CloudQueue,
                                    "Zuse Cloud",
                                    "Hosted chats and agent connections",
                                ) {
                                    if (workspace.signedIn) onCloud() else page = "account"
                                }
                            }
                            SettingsGroup {
                                if (workspace.canManageChats)
                                    SettingsRow(
                                        Icons.Outlined.Archive,
                                        "Archived chats",
                                        "Restore chats on this computer",
                                    ) {
                                        page = "archives"
                                    }
                                SettingsRow(
                                    Icons.AutoMirrored.Outlined.HelpOutline,
                                    "Help",
                                    "Pairing, zuse serve and connections",
                                ) {
                                    page = "help"
                                }
                            }
                            Text(
                                "Zuse for Android · ${BuildConfig.VERSION_NAME}",
                                Modifier.fillMaxWidth().padding(top = 4.dp),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                            )
                        }
                        "archives" -> {
                            if (!workspace.connected) Text("Reconnect to view archived chats.")
                            else if (workspace.archivesLoading)
                                LinearProgressIndicator(Modifier.fillMaxWidth())
                            else if (workspace.archivedSessions.isEmpty())
                                Text("No archived chats on this computer.")
                            workspace.archivedSessions.forEach { session ->
                                Row(
                                    Modifier.fillMaxWidth().padding(vertical = 6.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    Column(Modifier.weight(1f)) {
                                        Text(
                                            session.title,
                                            style = MaterialTheme.typography.bodyMedium,
                                        )
                                        Text(
                                            workspace.projects
                                                .firstOrNull { it.id == session.projectId }
                                                ?.name ?: "Computer",
                                            style = MaterialTheme.typography.labelSmall,
                                        )
                                    }
                                    TextButton(
                                        enabled =
                                            workspace.canManageChats && !workspace.chatActionBusy,
                                        onClick = { workspace.setChatArchived(session, false) },
                                    ) {
                                        Text("Restore")
                                    }
                                }
                            }
                            CompactAction(
                                "Refresh archives",
                                workspace.canManageChats && !workspace.archivesLoading,
                            ) {
                                workspace.loadArchivedChats()
                            }
                        }
                        "account" -> {
                            SettingsGroup {
                                SettingsRow(
                                    Icons.Outlined.AccountCircle,
                                    if (workspace.signedIn) "Signed in to Zuse"
                                    else "Sign in to Zuse",
                                    "Cloud chats and your linked computers",
                                    chevron = false,
                                )
                            }
                            Text(
                                "Use your Zuse account to reach linked computers and start chats in the cloud.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                CompactAction(
                                    if (workspace.signedIn) "Refresh account" else "Sign in",
                                    !workspace.cloudLoading,
                                ) {
                                    if (workspace.signedIn) workspace.refreshCloud()
                                    else workspace.signIn(openUri)
                                }
                                if (workspace.signedIn)
                                    CompactAction("Sign out") { workspace.signOut() }
                            }
                            if (workspace.signedIn)
                                SettingsGroup {
                                    SettingsRow(
                                        Icons.Outlined.CloudQueue,
                                        "Cloud agent settings",
                                        "Connect Claude, Codex and other agents",
                                        onClick = onCloud,
                                    )
                                }
                        }
                        "computers" -> {
                            SettingsGroup {
                                SettingsRow(
                                    Icons.Outlined.QrCodeScanner,
                                    "Pair a computer",
                                    "Scan a code or paste a pairing link",
                                ) {
                                    page = "pair"
                                }
                                SettingsRow(
                                    Icons.Outlined.Link,
                                    "Add manually",
                                    "LAN, remote server or Tailscale",
                                ) {
                                    page = "manual"
                                }
                            }
                            if (
                                workspace.devices.isNotEmpty() || workspace.computers.isNotEmpty()
                            ) {
                                SectionLabel("Saved computers")
                                SettingsGroup {
                                    workspace.devices.forEach { device ->
                                        Row(
                                            Modifier.fillMaxWidth(),
                                            verticalAlignment = Alignment.CenterVertically,
                                        ) {
                                            Box(Modifier.weight(1f)) {
                                                SettingsRow(
                                                    Icons.Outlined.Laptop,
                                                    device.label,
                                                    workspace.linkSummary(device) ?: device.address,
                                                    chevron = false,
                                                ) {
                                                    workspace.connect(Source.Device(device))
                                                    onClose()
                                                }
                                            }
                                            SmallIcon(
                                                Icons.Outlined.DeleteOutline,
                                                "Forget ${device.label}",
                                                { workspace.forgetDevice(device) },
                                            )
                                        }
                                    }
                                    workspace.computers.forEach { computer ->
                                        SettingsRow(
                                            Icons.Outlined.Computer,
                                            computer.label ?: "Computer",
                                            computer.providerKind,
                                        ) {
                                            workspace.connect(Source.Computer(computer))
                                            onClose()
                                        }
                                    }
                                }
                            }
                            if (workspace.source != null)
                                CompactAction("Disconnect", onClick = workspace::disconnect)
                            SectionLabel("Nearby computers")
                            Text(
                                nearbyError
                                    ?: if (nearby.isEmpty()) "Looking for Zuse on your Wi-Fi…"
                                    else "Select your computer to pair.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            if (nearby.isNotEmpty())
                                SettingsGroup {
                                    nearby.forEach { computer ->
                                        SettingsRow(Icons.Outlined.Laptop, computer.name) {
                                            nearbySelection = computer
                                            page = "pair"
                                        }
                                    }
                                }
                        }
                        "pair" -> {
                            Text(
                                "Open Zuse → Devices on your computer, or run zuse serve. Scan its QR code or paste the pairing link.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                CompactAction("Scan QR code") {
                                    scanner.launch(
                                        ScanOptions()
                                            .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                                            .setPrompt("Scan a Zuse pairing code")
                                            .setBeepEnabled(false)
                                            .setOrientationLocked(false)
                                    )
                                }
                            }
                            Column {
                                CompactField(
                                    "Computer name",
                                    label,
                                    { label = it },
                                    placeholder = "My computer",
                                )
                                CompactField(
                                    "Pairing link",
                                    pairing,
                                    { pairing = it },
                                    secret = true,
                                    placeholder = "Paste the link from Zuse",
                                )
                                TextButton(
                                    onClick = { advanced = !advanced },
                                    contentPadding = PaddingValues(0.dp),
                                    modifier = Modifier.height(28.dp),
                                ) {
                                    Text(
                                        if (advanced) "Hide certificate pin"
                                        else "Certificate pin (optional)",
                                        style = MaterialTheme.typography.labelSmall,
                                    )
                                }
                                if (advanced) CompactField("Certificate pin", pin, { pin = it })
                                CompactAction(
                                    if (workspace.busy) "Pairing…" else "Pair computer",
                                    pairing.isNotBlank() && !workspace.busy,
                                ) {
                                    workspace.pair(pairing, label, pin.ifBlank { null }, onClose)
                                }
                            }
                            nearbySelection?.let { computer ->
                                SettingsGroup {
                                    SettingsRow(
                                        Icons.Outlined.Laptop,
                                        computer.name,
                                        "Nearby computer",
                                        chevron = false,
                                    )
                                }
                                SelectionContainerText("Certificate: ${computer.pin}")
                                CompactField(
                                    "One-time pairing code",
                                    code,
                                    { code = it },
                                    secret = true,
                                )
                                CompactAction(
                                    "Pair ${computer.name}",
                                    code.isNotBlank() && !workspace.busy,
                                ) {
                                    workspace.pair(
                                        "${computer.address}/#pair=${Uri.encode(code)}",
                                        computer.name,
                                        computer.pin,
                                        onClose,
                                    )
                                }
                            }
                        }
                        "manual" -> {
                            Text(
                                "Use a LAN or Tailscale address, or a public server with TLS. Tailscale connects through your phone’s system VPN.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Column {
                                CompactField(
                                    "Computer name",
                                    label,
                                    { label = it },
                                    placeholder = "My computer",
                                )
                                CompactField(
                                    "WebSocket address",
                                    address,
                                    { address = it },
                                    placeholder = "wss://computer.example/rpc",
                                )
                                CompactField(
                                    "Connection token",
                                    token,
                                    { token = it },
                                    secret = true,
                                )
                                CompactField("Certificate pin (optional)", pin, { pin = it })
                                CompactAction(
                                    "Save and connect",
                                    address.isNotBlank() && label.isNotBlank(),
                                ) {
                                    workspace.saveDevice(
                                        label,
                                        address,
                                        token,
                                        pin.ifBlank { null },
                                    )
                                }
                            }
                        }
                        else -> {
                            SettingsGroup {
                                SettingsRow(
                                    Icons.Outlined.Computer,
                                    "Your computer",
                                    "Chats run on the computer you connect. Projects are its repositories.",
                                    chevron = false,
                                )
                                SettingsRow(
                                    Icons.Outlined.CloudQueue,
                                    "Cloud chats",
                                    "A cloud workspace is a hosted computer running a repository and agent.",
                                    chevron = false,
                                )
                            }
                            Text(
                                "For LAN or Tailscale, run zuse serve on your computer and add its connection in Computers. Connect Tailscale on both devices first.",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            CompactAction("Set up a computer") { page = "computers" }
                            CompactAction("Contact support") {
                                openUri(Uri.parse("mailto:hi@zuse.sh"))
                            }
                        }
                    }
                    Spacer(Modifier.height(16.dp))
                }
            }
        }
    }
}

@Composable
fun SheetHandle() {
    Box(Modifier.fillMaxWidth().height(20.dp), contentAlignment = Alignment.Center) {
        Box(
            Modifier.size(28.dp, 3.dp)
                .background(
                    MaterialTheme.colorScheme.onSurface.copy(alpha = .18f),
                    RoundedCornerShape(2.dp),
                )
        )
    }
}

@Composable
private fun SettingsGroup(content: @Composable ColumnScope.() -> Unit) {
    Surface(shape = RoundedCornerShape(12.dp), color = MaterialTheme.colorScheme.surfaceContainer) {
        Column(Modifier.fillMaxWidth(), content = content)
    }
}

@Composable
private fun SettingsRow(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    title: String,
    subtitle: String? = null,
    chevron: Boolean = true,
    onClick: (() -> Unit)? = null,
) {
    Row(
        Modifier.fillMaxWidth()
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .heightIn(min = 68.dp)
            .padding(horizontal = 20.dp, vertical = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(icon, null, Modifier.size(19.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, style = MaterialTheme.typography.bodyMedium)
            if (subtitle != null)
                Text(
                    subtitle,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = if (onClick == null) 3 else 1,
                    overflow = TextOverflow.Ellipsis,
                )
        }
        if (chevron && onClick != null)
            Icon(
                Icons.Outlined.ChevronRight,
                null,
                Modifier.size(16.dp),
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
            )
    }
}

@Composable
private fun SelectionContainerText(text: String) {
    androidx.compose.foundation.text.selection.SelectionContainer {
        Text(text, style = MaterialTheme.typography.labelSmall, fontFamily = FontFamily.Monospace)
    }
}

@Composable
fun DiffSheet(workspace: WorkspaceViewModel, onClose: () -> Unit) {
    var browse by rememberSaveable { mutableStateOf(false) }
    if (browse) { FileBrowserSheet(workspace) { browse = false }; return }
    var scope by rememberSaveable { mutableStateOf("unstaged") }
    var file by remember { mutableStateOf<String?>(null) }
    file?.let { path -> FileDialog(workspace, path) { file = null } }
    LaunchedEffect(scope) { workspace.loadDiff(scope) }
    DisposableEffect(Unit) { onDispose { workspace.closeDiff() } }
    ModalBottomSheet(
        onDismissRequest = onClose,
        sheetMaxWidth = 620.dp,
        dragHandle = { SheetHandle() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surface,
    ) {
        Column(Modifier.fillMaxHeight(.92f).fillMaxWidth()) {
            PaneHeader(
                "Changes",
                actions = {
                    SmallIcon(Icons.Outlined.Folder, "Browse all files", { browse = true })
                    SmallIcon(
                        Icons.Outlined.Refresh,
                        "Refresh changes",
                        { workspace.loadDiff(scope) },
                    )
                    SmallIcon(Icons.Outlined.Close, "Close changes", onClose)
                },
            )
            val scopes = listOf("unstaged", "staged", "branch")
            CompactTabs(listOf("Working tree", "Staged", "Branch"), scopes.indexOf(scope)) {
                scope = scopes[it]
            }
            workspace.error?.let {
                Text(
                    it,
                    Modifier.padding(horizontal = 16.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )
            }
            if (workspace.diffLoading) LinearProgressIndicator(Modifier.fillMaxWidth())
            LazyColumn(
                Modifier.weight(1f).fillMaxWidth(),
                contentPadding = PaddingValues(vertical = 12.dp),
            ) {
                workspace.patches.forEach { patch ->
                    item(key = "path:${patch.path}") {
                        Text(
                            patch.path,
                            Modifier.clickable { file = patch.path }
                                .fillMaxWidth()
                                .background(MaterialTheme.colorScheme.surfaceContainerHigh)
                                .padding(12.dp),
                            style = MaterialTheme.typography.labelMedium,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                    if (patch.error != null || patch.mode == "binary" || patch.truncated)
                        item(key = "note:${patch.path}") {
                            Text(
                                patch.error
                                    ?: if (patch.mode == "binary") "Binary file changed"
                                    else "Patch truncated by server",
                                Modifier.padding(12.dp),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    itemsIndexed(
                        patch.lines,
                        key = { index, _ -> "${patch.path}:$index" },
                        contentType = { _, _ -> "diff" },
                    ) { _, line ->
                        val color =
                            when (line.kind) {
                                DiffLine.Kind.ADDITION -> Color(0x24269A3B)
                                DiffLine.Kind.DELETION -> Color(0x24D93F4C)
                                else -> Color.Transparent
                            }
                        Row(
                            Modifier.fillMaxWidth()
                                .background(color)
                                .padding(horizontal = 8.dp, vertical = 2.dp)
                        ) {
                            Text(
                                line.oldLine?.toString().orEmpty(),
                                Modifier.width(34.dp),
                                style =
                                    MaterialTheme.typography.labelSmall.copy(
                                        fontSize = 10.sp,
                                        lineHeight = 20.sp,
                                    ),
                                fontFamily = FontFamily.Monospace,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            Text(
                                line.newLine?.toString().orEmpty(),
                                Modifier.width(34.dp),
                                style =
                                    MaterialTheme.typography.labelSmall.copy(
                                        fontSize = 10.sp,
                                        lineHeight = 20.sp,
                                    ),
                                fontFamily = FontFamily.Monospace,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                            androidx.compose.foundation.text.selection.SelectionContainer {
                                Text(
                                    line.text,
                                    fontFamily = FontFamily.Monospace,
                                    style =
                                        MaterialTheme.typography.bodySmall.copy(
                                            fontSize = 11.sp,
                                            lineHeight = 20.sp,
                                        ),
                                    color =
                                        if (line.kind == DiffLine.Kind.HEADER) Color(0xFFA63AA5)
                                        else MaterialTheme.colorScheme.onSurface,
                                    modifier = Modifier.horizontalScroll(rememberScrollState()),
                                )
                            }
                        }
                    }
                }
                if (!workspace.diffLoading && workspace.patches.isEmpty())
                    item {
                        Text(
                            "No changes",
                            Modifier.padding(24.dp),
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
            }
        }
    }
}

@Composable
internal fun FileDialog(workspace: WorkspaceViewModel, path: String, onClose: () -> Unit) {
    val openedSource = remember { workspace.source to workspace.selectedSession?.id }
    LaunchedEffect(workspace.source, workspace.selectedSession?.id) {
        if (openedSource != (workspace.source to workspace.selectedSession?.id)) onClose()
    }
    var lines by remember(path) { mutableStateOf<List<String>?>(null) }
    var failure by remember(path) { mutableStateOf<String?>(null) }
    LaunchedEffect(path) {
        try {
            lines = workspace.readFile(path)
        } catch (cancelled: kotlinx.coroutines.CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            failure = error.message
        }
    }
    androidx.compose.ui.window.Dialog(
        onDismissRequest = onClose,
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Surface(
            Modifier.fillMaxSize().padding(16.dp).widthIn(max = 1100.dp),
            shape = RoundedCornerShape(12.dp),
        ) {
            Column {
                PaneHeader(
                    path,
                    actions = { SmallIcon(Icons.Outlined.Close, "Close file", onClose) },
                )
                if (failure != null)
                    Text(
                        failure.orEmpty(),
                        Modifier.padding(16.dp),
                        color = MaterialTheme.colorScheme.error,
                    )
                else if (lines == null) CircularProgressIndicator(Modifier.padding(16.dp))
                else
                    LazyColumn(Modifier.weight(1f), contentPadding = PaddingValues(12.dp)) {
                        itemsIndexed(lines.orEmpty()) { index, text ->
                            Row(Modifier.padding(vertical = 2.dp)) {
                                Text(
                                    "${index + 1}",
                                    Modifier.width(40.dp),
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                                androidx.compose.foundation.text.selection.SelectionContainer {
                                    Text(
                                        text.ifEmpty { " " },
                                        fontFamily = FontFamily.Monospace,
                                        style = MaterialTheme.typography.bodySmall,
                                        modifier = Modifier.horizontalScroll(rememberScrollState()),
                                    )
                                }
                            }
                        }
                    }
            }
        }
    }
}

@Composable
fun CompactTabs(labels: List<String>, selected: Int, onSelect: (Int) -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        labels.forEachIndexed { index, label ->
            Surface(
                shape = RoundedCornerShape(8.dp),
                color =
                    if (selected == index) MaterialTheme.colorScheme.secondaryContainer
                    else Color.Transparent,
                modifier =
                    Modifier.height(28.dp)
                        .clickable { onSelect(index) }
                        .semantics {
                            role = Role.Tab
                            this.selected = index == selected
                        },
            ) {
                Box(Modifier.padding(horizontal = 12.dp), contentAlignment = Alignment.Center) {
                    Text(
                        label,
                        style = MaterialTheme.typography.labelMedium,
                        color =
                            if (selected == index) MaterialTheme.colorScheme.onSurface
                            else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}
