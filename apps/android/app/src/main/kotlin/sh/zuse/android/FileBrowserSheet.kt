package sh.zuse.android

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.JsonElement
import sh.zuse.core.*

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FileBrowserSheet(workspace: WorkspaceViewModel, onClose: () -> Unit) {
    key(workspace.source, workspace.selectedSession?.id) { WorkspaceFileBrowser(workspace, onClose) }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun WorkspaceFileBrowser(workspace: WorkspaceViewModel, onClose: () -> Unit) {
    var directory by rememberSaveable { mutableStateOf("") }
    var file by remember { mutableStateOf<String?>(null) }
    var revision by remember { mutableIntStateOf(0) }
    var requestedCursor by remember { mutableStateOf<String?>(null) }
    var nextCursor by remember { mutableStateOf<String?>(null) }
    var rows by remember { mutableStateOf<List<JsonElement>>(emptyList()) }
    var loading by remember { mutableStateOf(false) }
    var failure by remember { mutableStateOf<String?>(null) }
    val session = workspace.selectedSession?.id
    file?.let { path -> FileDialog(workspace, path) { file = null } }
    LaunchedEffect(directory, requestedCursor, revision, session) {
        loading = true
        failure = null
        if (requestedCursor == null) rows = emptyList()
        try {
            val page = workspace.listDirectory(directory, requestedCursor)
            rows = (if (requestedCursor == null) emptyList() else rows) + page.array("entries")
            rows = rows.distinctBy { it.text("path") }
            nextCursor = page.text("nextCursor")
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (error: Exception) { failure = error.message ?: "Could not load files." }
        finally { loading = false }
    }
    ModalBottomSheet(onDismissRequest = onClose, sheetMaxWidth = 620.dp,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = MaterialTheme.colorScheme.surface) {
        Column(Modifier.fillMaxHeight(.92f).fillMaxWidth()) {
            PaneHeader("Files", directory.ifBlank { "Project root" }, leading = {
                SmallIcon(Icons.AutoMirrored.Outlined.ArrowBack, "Parent folder", {
                    if (directory.isEmpty()) onClose() else {
                        requestedCursor = null; directory = directory.substringBeforeLast('/', "")
                    }
                })
            }, actions = {
                SmallIcon(Icons.Outlined.Refresh, "Refresh files", { requestedCursor = null; revision++ })
                SmallIcon(Icons.Outlined.Close, "Close files", onClose)
            })
            LazyColumn(Modifier.weight(1f).fillMaxWidth().semantics { contentDescription = "Workspace files" }) {
                items(rows, key = { it.text("path").orEmpty() }) { entry ->
                    val folder = entry.text("kind") == "directory"
                    ListItem(headlineContent = { Text(entry.text("name").orEmpty()) },
                        supportingContent = { if (folder) Text("Folder") },
                        modifier = Modifier.clickable {
                            val path = entry.text("path") ?: return@clickable
                            if (folder) { requestedCursor = null; directory = path } else file = path
                        })
                }
                if (failure != null) item { Text(failure!!, Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error) }
                if (loading) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
                else if (nextCursor != null && failure == null) item {
                    TextButton(onClick = { requestedCursor = nextCursor }) { Text("Load more files") }
                }
                else if (rows.isEmpty() && failure == null) item { Text("This folder is empty.", Modifier.padding(16.dp)) }
            }
        }
    }
}
