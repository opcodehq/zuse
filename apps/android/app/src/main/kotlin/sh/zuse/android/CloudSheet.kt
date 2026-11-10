@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package sh.zuse.android

import android.net.Uri
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.*
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import sh.zuse.core.*

@Composable
private fun CloudPicker(
    label: String,
    selected: String,
    rows: List<Pair<String, String>>,
    onSelect: (String) -> Unit,
) {
    var expanded by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
        Text(
            label,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Box {
            TextButton(
                onClick = { expanded = true },
                modifier = Modifier.height(28.dp),
                contentPadding = PaddingValues(horizontal = 4.dp),
            ) {
                Text(rows.firstOrNull { it.first == selected }?.second ?: "Choose…", maxLines = 1)
                Icon(Icons.Outlined.ExpandMore, null, Modifier.size(16.dp))
            }
            DropdownMenu(expanded, { expanded = false }) {
                rows.forEach { (id, title) ->
                    DropdownMenuItem(
                        text = { Text(title) },
                        onClick = {
                            onSelect(id)
                            expanded = false
                        },
                    )
                }
            }
        }
    }
}

@Composable
fun CloudSheet(
    workspace: WorkspaceViewModel,
    openUri: (Uri) -> Unit,
    onClose: () -> Unit,
    cloud: CloudViewModel = viewModel(),
) {
    var tab by rememberSaveable { mutableStateOf(0) }
    var project by rememberSaveable { mutableStateOf("") }
    var provider by rememberSaveable { mutableStateOf("") }
    var size by rememberSaveable { mutableStateOf("") }
    var agent by rememberSaveable { mutableStateOf("") }
    var model by rememberSaveable { mutableStateOf("") }
    var repository by rememberSaveable { mutableStateOf("") }
    var branch by rememberSaveable { mutableStateOf("main") }
    var privateRepo by rememberSaveable { mutableStateOf(true) }
    var addRepository by rememberSaveable { mutableStateOf(false) }
    var credentialProvider by rememberSaveable { mutableStateOf("claude") }
    var method by rememberSaveable { mutableStateOf("api-key") }
    var credential by remember { mutableStateOf("") }
    LaunchedEffect(Unit) { cloud.load() }
    LaunchedEffect(cloud.pendingRepository) {
        if (cloud.pendingRepository != kotlinx.serialization.json.JsonNull) {
            repository = cloud.pendingRepository.text("repositoryUrl").orEmpty()
            branch = cloud.pendingRepository.text("defaultBranch") ?: "main"
            privateRepo = cloud.pendingRepository.text("visibility") != "public"
            addRepository = true
        }
    }
    LaunchedEffect(cloud.pendingLaunch) {
        if (cloud.pendingLaunch != kotlinx.serialization.json.JsonNull) {
            val pending = cloud.pendingLaunch
            project = pending.text("projectId").orEmpty()
            provider = pending.text("providerId").orEmpty()
            size = pending.text("sizeId").orEmpty()
            agent = pending.text("agent").orEmpty()
            model = pending.text("model").orEmpty()
        }
    }
    LaunchedEffect(cloud.providers) {
        if (provider.isEmpty())
            provider = cloud.providers.firstOrNull()?.text("providerId").orEmpty()
    }
    LaunchedEffect(cloud.agents) {
        if (agent.isEmpty()) {
            agent = cloud.agents.firstOrNull()?.provider.orEmpty()
            model = cloud.agents.firstOrNull()?.id.orEmpty()
        }
    }
    ModalBottomSheet(
        onDismissRequest = { if (!cloud.busy) onClose() },
        sheetMaxWidth = 560.dp,
        dragHandle = { SheetHandle() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surface,
    ) {
        Column(
            Modifier.fillMaxHeight(.9f)
                .fillMaxWidth()
                .widthIn(max = 620.dp)
                .align(Alignment.CenterHorizontally)
        ) {
            PaneHeader(
                "Zuse Cloud",
                actions = {
                    SmallIcon(
                        Icons.Outlined.Refresh,
                        "Refresh cloud",
                        { cloud.load() },
                        !cloud.busy,
                    )
                    SmallIcon(Icons.Outlined.Close, "Close cloud", onClose, !cloud.busy)
                },
            )
            CompactTabs(listOf("New cloud chat", "Cloud providers"), tab) { tab = it }
            if (cloud.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            Column(
                Modifier.weight(1f)
                    .verticalScroll(rememberScrollState())
                    .padding(16.dp)
                    .imePadding()
            ) {
                cloud.failure?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                if (tab == 0) {
                    CloudPicker(
                        "Repository",
                        project,
                        cloud.projects.mapNotNull {
                            it.text("projectId")?.let { id -> id to (it.text("displayName") ?: id) }
                        },
                    ) {
                        project = it
                    }
                    TextButton(onClick = { addRepository = !addRepository }) {
                        Text("Connect a repository")
                    }
                    if (addRepository) {
                        CompactField("Repository URL", repository, { repository = it })
                        CompactField("Default branch", branch, { branch = it })
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(privateRepo, { privateRepo = it })
                            Text("Private repository", style = MaterialTheme.typography.bodySmall)
                        }
                        CompactAction(
                            "Connect repository",
                            !cloud.busy && repository.isNotBlank() && branch.isNotBlank(),
                        ) {
                            cloud.connectRepository(repository, branch, privateRepo) {
                                project = it
                                repository = ""
                                addRepository = false
                            }
                        }
                    }
                    CloudPicker(
                        "Cloud provider",
                        provider,
                        cloud.providers.mapNotNull {
                            it.text("providerId")?.let { id ->
                                id to (it.text("displayName") ?: id)
                            }
                        },
                    ) {
                        provider = it
                        size = ""
                    }
                    val sizes =
                        cloud.providers
                            .firstOrNull { it.text("providerId") == provider }
                            ?.array("sizes")
                            .orEmpty()
                    CloudPicker(
                        "Size",
                        size,
                        listOf("" to "Default size") +
                            sizes.mapNotNull {
                                it.text("sizeId")?.let { id ->
                                    id to (it.text("displayName") ?: id)
                                }
                            },
                    ) {
                        size = it
                    }
                    CloudPicker(
                        "Agent",
                        agent,
                        cloud.agents
                            .map { it.provider }
                            .distinct()
                            .map { it to it.replaceFirstChar(Char::uppercaseChar) },
                    ) {
                        agent = it
                        model =
                            cloud.agents
                                .firstOrNull { choice -> choice.provider == it }
                                ?.id
                                .orEmpty()
                    }
                    CloudPicker(
                        "Model",
                        model,
                        cloud.agents.filter { it.provider == agent }.map { it.id to it.name },
                    ) {
                        model = it
                    }
                    if (cloud.agents.isEmpty())
                        TextButton(onClick = { tab = 1 }) {
                            Text("Connect an agent to start a cloud chat")
                        }
                    Text(
                        "Runs this repository and agent on a hosted computer using your cloud plan or provider account.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    CompactAction(
                        "Start cloud chat",
                        !cloud.busy &&
                            project.isNotEmpty() &&
                            provider.isNotEmpty() &&
                            model.isNotEmpty(),
                    ) {
                        val selected =
                            cloud.projects.firstOrNull { it.text("projectId") == project }
                        val selectedModel =
                            cloud.agents.firstOrNull { it.provider == agent && it.id == model }
                        if (selected != null && selectedModel != null)
                            cloud.launch(selected, provider, size, selectedModel) {
                                workspace.connect(Source.Cloud(it))
                                workspace.refreshCloud()
                                onClose()
                            }
                    }
                } else {
                    cloud.auth.array("providers").forEach { row ->
                        ListItem(
                            headlineContent = { Text(row.text("providerId") ?: "Agent") },
                            supportingContent = {
                                Text(row.text("accountLabel") ?: row.text("state") ?: "Unknown")
                            },
                            colors =
                                ListItemDefaults.colors(
                                    containerColor = MaterialTheme.colorScheme.surface
                                ),
                        )
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        CompactAction("Sign in to Codex", !cloud.busy) { cloud.login("codex") }
                        CompactAction("Sign in to Grok", !cloud.busy) { cloud.login("grok") }
                    }
                    if (cloud.operation.text("state") == "authorizing") {
                        SelectionContainer {
                            Text(
                                cloud.operation.text("verificationCode").orEmpty(),
                                style = MaterialTheme.typography.titleMedium,
                            )
                        }
                        cloud.operation.text("verificationUrl")?.let { address ->
                            if (address.startsWith("https://"))
                                TextButton(onClick = { openUri(Uri.parse(address)) }) {
                                    Text("Open sign-in page")
                                }
                        }
                        Text("Waiting for sign-in…", style = MaterialTheme.typography.bodySmall)
                        TextButton(onClick = { cloud.cancelLogin() }) { Text("Cancel sign-in") }
                    }
                    Spacer(Modifier.height(16.dp))
                    Text("Connect with a credential", style = MaterialTheme.typography.titleSmall)
                    CloudPicker(
                        "Agent",
                        credentialProvider,
                        listOf("claude", "codex", "cursor", "grok").map {
                            it to it.replaceFirstChar(Char::uppercaseChar)
                        },
                    ) {
                        credentialProvider = it
                    }
                    CloudPicker(
                        "Authentication",
                        method,
                        listOf("api-key" to "API key", "subscription" to "Subscription token"),
                    ) {
                        method = it
                    }
                    CompactField("Credential", credential, { credential = it }, secret = true)
                    CompactAction("Connect agent", !cloud.busy && credential.isNotBlank()) {
                        cloud.configure(credentialProvider, method, credential) { credential = "" }
                    }
                    Text(
                        "Credentials are encrypted on this device for your cloud authentication service.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}
