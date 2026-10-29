package sh.zuse.android

import android.content.Intent
import android.net.ConnectivityManager
import android.net.Network
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels

class MainActivity : ComponentActivity() {
    private val workspace: WorkspaceViewModel by viewModels()
    private val connectivity by lazy { getSystemService(ConnectivityManager::class.java) }
    private val networkCallback =
        object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                runOnUiThread { workspace.networkRecovered() }
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        intent.data?.let(workspace::handleLink)
        connectivity.registerDefaultNetworkCallback(networkCallback)
        setContent {
            ZuseTheme {
                ZuseApp(workspace, openUri = { startActivity(Intent(Intent.ACTION_VIEW, it)) })
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.data?.let(workspace::handleLink)
    }

    override fun onStart() {
        super.onStart()
        workspace.setForeground(true)
    }

    override fun onStop() {
        workspace.setForeground(false)
        super.onStop()
    }

    override fun onDestroy() {
        connectivity.unregisterNetworkCallback(networkCallback)
        super.onDestroy()
    }
}
