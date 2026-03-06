package com.example.zingolibffi

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.example.zingolibffi.ui.theme.ZingolibFfiTheme
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.random.Random

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            ZingolibFfiTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    ViewOnlyKitchenSinkUiOnly()
                }
            }
        }
    }
}

@Composable
fun ViewOnlyKitchenSinkUiOnly() {
    val scope = rememberCoroutineScope()
    val scroll = rememberScrollState()

    // Hard-coded for now
    var indexerUri by remember { mutableStateOf("http://127.0.0.1:16992") }
    var birthday by remember { mutableStateOf("1") }
    var ufvk by remember {
        mutableStateOf("")
    }

    // UI-only state
    var engineCreated by remember { mutableStateOf(false) }
    var walletLoaded by remember { mutableStateOf(false) }
    var syncing by remember { mutableStateOf(false) }

    var walletHeight by remember { mutableStateOf(0) }
    var networkHeight by remember { mutableStateOf(0) }
    var percent by remember { mutableStateOf(0f) }

    var confirmed by remember { mutableStateOf("0") }
    var total by remember { mutableStateOf("0") }

    var lastError by remember { mutableStateOf<String?>(null) }
    var log by remember { mutableStateOf("") }

    fun appendLog(line: String) {
        log = (log + line + "\n").takeLast(40_000)
    }

    // UI-only "sync loop"
    LaunchedEffect(syncing) {
        if (!syncing) return@LaunchedEffect
        while (syncing) {
            delay(500)

            // simulate network tip moving sometimes TODO: unmock
            if (Random.nextFloat() < 0.35f) {
                networkHeight += 1
            }

            // simulate wallet catching up TODO unmock
            if (walletHeight < networkHeight) {
                walletHeight += 1
            }

            percent = if (networkHeight > 0) {
                (walletHeight.toFloat() / networkHeight.toFloat()).coerceIn(0f, 1f)
            } else 0f

            appendLog("[event] SyncProgress wh=$walletHeight nh=$networkHeight pct=${"%.3f".format(percent)}")


            if (Random.nextFloat() < 0.15f) {
                val c = Random.nextInt(0, 5)
                val t = c + Random.nextInt(0, 3)
                confirmed = c.toString()
                total = t.toString()
                appendLog("[event] BalanceChanged confirmed=$confirmed total=$total")
            }


            if (walletHeight >= networkHeight) {
                appendLog("[event] SyncFinished")
            }
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(16.dp)
            .verticalScroll(scroll),
        verticalArrangement = Arrangement.spacedBy(12.dp)
    ) {
        Text("View-only Wallet – Kitchen Sink (UI only)", style = MaterialTheme.typography.headlineSmall)

        Card {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Inputs", style = MaterialTheme.typography.titleMedium)

                OutlinedTextField(
                    value = indexerUri,
                    onValueChange = { indexerUri = it },
                    label = { Text("Indexer URI") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth()
                )

                OutlinedTextField(
                    value = birthday,
                    onValueChange = { birthday = it.filter(Char::isDigit) },
                    label = { Text("Birthday (u32)") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth()
                )

                OutlinedTextField(
                    value = ufvk,
                    onValueChange = { ufvk = it },
                    label = { Text("UFVK (view-only key)") },
                    modifier = Modifier.fillMaxWidth(),
                    minLines = 3
                )
            }
        }

        Card {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Actions", style = MaterialTheme.typography.titleMedium)

                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp)
                ) {
                    Button(
                        enabled = !engineCreated,
                        onClick = {
                            lastError = null
                            engineCreated = true
                            appendLog("[ui] Engine created")
                        }
                    ) { Text("Create Engine") }

                    OutlinedButton(
                        enabled = engineCreated,
                        onClick = {
                            lastError = null
                            appendLog("[ui] Listener set")
                        }
                    ) { Text("Set Listener") }

                    OutlinedButton(
                        enabled = engineCreated,
                        onClick = {
                            lastError = null
                            appendLog("[ui] Listener cleared")
                        }
                    ) { Text("Clear Listener") }
                }

                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Button(
                        enabled = engineCreated && !walletLoaded,
                        onClick = {
                            lastError = null
                            walletLoaded = true
                            walletHeight = 0
                            networkHeight = 0
                            percent = 0f
                            confirmed = "0"
                            total = "0"
                            appendLog("[ui] Wallet initialized (view-only)")
                            appendLog("[event] EngineReady")
                        }
                    ) { Text("Init View-only") }

                    OutlinedButton(
                        enabled = engineCreated && walletLoaded && !syncing,
                        onClick = {
                            lastError = null
                            syncing = true
                            appendLog("[event] SyncStarted")
                            // kick network height so percent isn't NaN-like
                            if (networkHeight == 0) networkHeight = 10
                        }
                    ) { Text("Start Sync") }

                    OutlinedButton(
                        enabled = syncing,
                        onClick = {
                            lastError = null
                            syncing = false
                            appendLog("[event] SyncPaused")
                        }
                    ) { Text("Pause") }
                }

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(10.dp)
                ) {
                    OutlinedButton(
                        modifier = Modifier.weight(1f),
                        enabled = engineCreated && walletLoaded,
                        onClick = {
                            lastError = null
                            appendLog("[ui] get_network_height = $networkHeight")
                        }
                    ) { Text("Get Network Height", maxLines = 1, softWrap = false) }

                    OutlinedButton(
                        modifier = Modifier.weight(1f),
                        enabled = engineCreated && walletLoaded,
                        onClick = {
                            lastError = null
                            appendLog("[ui] get_balance_snapshot confirmed=$confirmed total=$total")
                        }
                    ) { Text("Get Balance", maxLines = 1, softWrap = false) }

                    Button(
                        modifier = Modifier.widthIn(min = 96.dp),
                        enabled = engineCreated,
                        colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error),
                        onClick = { /* ... */ }
                    ) {
                        Text("Unload", maxLines = 1, softWrap = false)
                    }
                }

                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    OutlinedButton(
                        enabled = true,
                        onClick = { log = "" }
                    ) { Text("Clear Log") }

                    OutlinedButton(
                        enabled = engineCreated,
                        onClick = {
                            lastError = null
                            syncing = false
                            walletLoaded = false
                            engineCreated = false
                            appendLog("[ui] Engine shutdown")
                        }
                    ) { Text("Shutdown") }
                }
            }
        }

        Card {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("Status", style = MaterialTheme.typography.titleMedium)
                Text("Engine: ${if (engineCreated) "created" else "none"}")
                Text("Wallet: ${if (walletLoaded) "loaded (view-only)" else "not loaded"}")
                Text("Syncing: $syncing")
                Text("Heights: wallet=$walletHeight network=$networkHeight")
                LinearProgressIndicator(
                    progress = { percent },
                    modifier = Modifier.fillMaxWidth()
                )
                Text("Balance: confirmed=$confirmed total=$total")

                if (lastError != null) {
                    Text("Last error: $lastError", color = MaterialTheme.colorScheme.error)
                }
            }
        }

        Card {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Log", style = MaterialTheme.typography.titleMedium)
                Text(
                    text = log.ifBlank { "(no events yet)" },
                    fontFamily = FontFamily.Monospace,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.fillMaxWidth()
                )
            }
        }

        Spacer(Modifier.height(24.dp))
    }
}
