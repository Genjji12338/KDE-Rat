package dev.tunnelrat.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Bedtime
import androidx.compose.material.icons.rounded.DesktopAccessDisabled
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material.icons.rounded.PowerSettingsNew
import androidx.compose.material.icons.rounded.RestartAlt
import androidx.compose.material.icons.rounded.WbSunny
import androidx.compose.material3.Icon
import androidx.compose.material.icons.rounded.QrCode2
import androidx.compose.material.icons.rounded.Slideshow
import androidx.compose.material.icons.rounded.WifiOff
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.TextButton
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import android.net.Uri
import dev.tunnelrat.Tura as TuraCore
import dev.tunnelrat.TuraViewModel
import dev.tunnelrat.ui.scan.ScannerScreen
import dev.tunnelrat.net.TuraClient
import dev.tunnelrat.proto.MouseButton
import dev.tunnelrat.proto.PowerAction
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import androidx.compose.runtime.rememberCoroutineScope

private val CardShape = RoundedCornerShape(24.dp)

@Composable
fun HomeScreen(vm: TuraViewModel, onMirror: () -> Unit = {}) {
    val state by vm.client.state.collectAsState()
    val pc by vm.pc.collectAsState()
    val offer by vm.offer.collectAsState()
    var scanning by remember { mutableStateOf(false) }
    var keyboard by remember { mutableStateOf(false) }
    var settings by remember { mutableStateOf(false) }
    val toast by vm.toast.collectAsState()
    val snackbar = remember { SnackbarHostState() }

    LaunchedEffect(toast) {
        toast?.let { snackbar.showSnackbar(it); vm.toastShown() }
    }

    Box(Modifier.fillMaxSize().background(Tura.Bg)) {
        Column(
            Modifier.fillMaxSize().safeDrawingPadding().padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            val net by vm.tura.net.collectAsState()
            val links by vm.tura.links.collectAsState()
            Header(
                state, net, paired = pc != null, pcName = pc?.name, links = links, selected = pc?.fingerprint,
                onSelect = vm.tura::select, onPairAnother = { scanning = true }, onUnpair = vm::unpair, onSettings = { settings = true }, onPresent = { vm.presenting.value = true }, onCast = { vm.casting.value = true },
                mirroring = vm.tura.mirroring.collectAsState().value, onMirror = { if (vm.tura.mirroring.value) vm.tura.stopMirror() else onMirror() })

            if (pc == null) {
                PairCard(Modifier.fillMaxWidth().weight(1f), onScan = { scanning = true })
                return@Column
            }

            if (state !is TuraClient.State.Connected) {
                when (net) {
                    TuraCore.Net.MobileOnly -> NetBanner(
                        "Your phone isn't on Wi-Fi",
                        "Turn on Wi-Fi to reach ${pc!!.name} at home. Away from home, connect Tailscale on the phone and Tunnelrat uses the PC's Tailscale address.",
                    )
                    TuraCore.Net.None -> NetBanner("No network", "Your phone is offline. Tunnelrat reconnects as soon as it's back.")
                    else -> (state as? TuraClient.State.Failed)?.let { f ->
                        Text("Can't reach ${pc!!.name} at ${f.host} — ${f.reason}", color = Tura.Muted, fontSize = 12.sp)
                    }
                }
            }

            val mirroring by vm.tura.mirroring.collectAsState()
            if (mirroring) MirrorBanner(pc!!.name, onStop = vm.tura::stopMirror)

            val pcInfo by vm.client.pcInfo.collectAsState()
            if (state is TuraClient.State.Connected && pcInfo?.desktop == false) {
                LoginCard(pc!!.name, busy = vm.tura.loggingIn.collectAsState().value, onLogin = vm.tura::logIn)
            }
            PowerRow(allowed = pcInfo?.powerActions, onWake = vm::wake, onPower = vm::power, onHint = vm::notify)

            val nowPlaying by vm.client.nowPlaying.collectAsState()
            MediaCard(
                np = nowPlaying,
                onMedia = { vm.media(it) },
                onVolume = { vm.volume(it) },
                onMute = { vm.mute() },
                modifier = Modifier.fillMaxWidth(),
            )

            Touchpad(
                onMove = { dx, dy -> vm.move(dx, dy) },
                onScroll = { dx, dy -> vm.scroll(dx, dy) },
                onLeftClick = { vm.click(MouseButton.Left) },
                onRightClick = { vm.click(MouseButton.Right) },
                onMiddleClick = { vm.click(MouseButton.Middle) },
                onSwipe = { vm.threeFinger(it) },
                modifier = Modifier.fillMaxWidth().weight(1f),
            )

            if (keyboard) {
                KeyboardBar(
                    onText = { t, m -> vm.type(t, m) },
                    onKey = { c, m -> vm.key(c, m) },
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            Row(Modifier.fillMaxWidth().height(56.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                MouseKey("L", Modifier.weight(1f)) { vm.click(MouseButton.Left) }
                MouseKey("M", Modifier.width(56.dp)) { vm.click(MouseButton.Middle) }
                MouseKey(if (keyboard) "✕" else "⌨", Modifier.width(56.dp), active = keyboard) { keyboard = !keyboard }
                MouseKey("R", Modifier.weight(1f)) { vm.click(MouseButton.Right) }
            }
        }
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).safeDrawingPadding().padding(bottom = 80.dp))

        val presenting by vm.presenting.collectAsState()
        if (presenting) {
            PresentationScreen(
                onKey = { vm.key(it) },
                onBlank = { vm.type("b") },
                onPointer = { dx, dy -> vm.move(dx, dy) },
                onClose = { vm.presenting.value = false },
            )
        }

        val casting by vm.casting.collectAsState()
        if (casting) {
            CastScreen(vm, pcName = pc?.name ?: "the PC", onClose = { vm.casting.value = false })
        }

        if (settings) {
            SettingsScreen(vm.tura, onUnpair = vm::unpair, onClose = { settings = false })
        }

        if (scanning) {
            ScannerScreen(
                onResult = { link ->
                    scanning = false
                    vm.onPairLink(Uri.parse(link))
                },
                onClose = { scanning = false },
            )
        }

        offer?.let { o ->
            AlertDialog(
                onDismissRequest = vm::declineOffer,
                containerColor = Tura.SurfaceHigh,
                title = { Text("Pair with ${o.pc.name}?", color = Tura.Text, fontWeight = FontWeight.Bold) },
                text = {
                    Text(
                        "This phone will be able to control ${o.pc.name}.\n\nPC key ${o.pc.fingerprint.take(8)}…${o.pc.fingerprint.takeLast(4)}",
                        color = Tura.Muted,
                    )
                },
                confirmButton = { TextButton(onClick = vm::acceptOffer) { Text("Pair", color = Tura.Amber, fontWeight = FontWeight.Bold) } },
                dismissButton = { TextButton(onClick = vm::declineOffer) { Text("Cancel", color = Tura.Muted) } },
            )
        }
    }
}

@Composable
private fun Header(state: TuraClient.State, net: TuraCore.Net, paired: Boolean, pcName: String?, links: List<TuraCore.PcLink>, selected: String?, onSelect: (String) -> Unit, onPairAnother: () -> Unit, onUnpair: () -> Unit, onSettings: () -> Unit, onPresent: () -> Unit, onCast: () -> Unit, mirroring: Boolean, onMirror: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("tunnelrat", color = Tura.Text, fontSize = 30.sp, fontWeight = FontWeight.Black, letterSpacing = (-1).sp)
        Spacer(Modifier.weight(1f))
        if (paired) {
            Box(
                Modifier.size(38.dp).clip(CircleShape).background(Tura.Surface).border(1.dp, Tura.Line, CircleShape).clickable(onClick = onPresent),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Rounded.Slideshow, contentDescription = "Presentation mode", tint = Tura.Text, modifier = Modifier.size(20.dp)) }
            Spacer(Modifier.width(8.dp))
        }
        val (label, dot) = when {
            !paired -> "not paired" to Tura.Muted
            state is TuraClient.State.Connected -> (state.hostname ?: pcName ?: "connected") to Tura.Ok
            net == TuraCore.Net.MobileOnly -> "no Wi-Fi" to Tura.Danger
            net == TuraCore.Net.None -> "no network" to Tura.Danger
            state is TuraClient.State.Connecting -> "${pcName ?: "PC"} · digging…" to Tura.Amber
            state is TuraClient.State.Failed -> "${pcName ?: "PC"} · offline" to Tura.Danger
            else -> "${pcName ?: "PC"} · offline" to Tura.Muted
        }
        Box {
            Row(
                Modifier.clip(CircleShape).background(Tura.Surface).border(1.dp, Tura.Line, CircleShape)
                    .clickable(enabled = paired) { menu = true }
                    .padding(horizontal = 12.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.size(8.dp).clip(CircleShape).background(dot))
                Spacer(Modifier.width(8.dp))
                Text(label, color = Tura.Text, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = Tura.SurfaceHigh) {
                // Paired PCs: tap to make it the one the phone controls.
                for (link in links) {
                    val pc by link.pc.collectAsState()
                    val st by link.client.state.collectAsState()
                    val on = st is TuraClient.State.Connected
                    DropdownMenuItem(
                        text = {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Box(Modifier.size(8.dp).clip(CircleShape).background(if (on) Tura.Ok else Tura.Muted))
                                Spacer(Modifier.width(10.dp))
                                Text(
                                    pc.name,
                                    color = Tura.Text,
                                    fontWeight = if (pc.fingerprint == selected) FontWeight.Bold else FontWeight.Normal,
                                )
                                if (pc.fingerprint == selected) Text("  ✓", color = Tura.Amber)
                            }
                        },
                        onClick = { menu = false; onSelect(pc.fingerprint) },
                    )
                }
                DropdownMenuItem(text = { Text("Pair another PC", color = Tura.Amber) }, onClick = { menu = false; onPairAnother() })
                androidx.compose.material3.HorizontalDivider(color = Tura.Line)
                DropdownMenuItem(text = { Text("PC screen", color = Tura.Text) }, onClick = { menu = false; onCast() })
                DropdownMenuItem(text = { Text(if (mirroring) "Stop showing phone on PC" else "Show phone on PC", color = Tura.Text) }, onClick = { menu = false; onMirror() })
                DropdownMenuItem(text = { Text("Presentation mode", color = Tura.Text) }, onClick = { menu = false; onPresent() })
                DropdownMenuItem(text = { Text("Settings", color = Tura.Text) }, onClick = { menu = false; onSettings() })
                DropdownMenuItem(text = { Text("Unpair ${pcName ?: "this PC"}", color = Tura.Danger) }, onClick = { menu = false; onUnpair() })
            }
        }
    }
}

/** The PC is at its login screen (e.g. just woken): type the password here. */
@Composable
private fun LoginCard(pcName: String, busy: Boolean, onLogin: (String) -> Unit) {
    var password by remember { mutableStateOf("") }
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Tura.Surface).border(1.dp, Tura.AmberDim, RoundedCornerShape(16.dp)).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text("$pcName is at the login screen", color = Tura.Text, fontWeight = FontWeight.SemiBold)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            androidx.compose.material3.OutlinedTextField(
                value = password,
                onValueChange = { password = it },
                singleLine = true,
                enabled = !busy,
                placeholder = { Text("Your PC password", color = Tura.Muted) },
                visualTransformation = androidx.compose.ui.text.input.PasswordVisualTransformation(),
                keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                    keyboardType = androidx.compose.ui.text.input.KeyboardType.Password,
                    imeAction = androidx.compose.ui.text.input.ImeAction.Go,
                ),
                keyboardActions = androidx.compose.foundation.text.KeyboardActions(onGo = { onLogin(password); password = "" }),
                modifier = Modifier.weight(1f),
            )
            TextButton(onClick = { onLogin(password); password = "" }, enabled = !busy && password.isNotEmpty()) {
                Text(if (busy) "Logging in…" else "Log in", color = Tura.Amber)
            }
        }
        Text("Typed into the PC's login screen over the paired, encrypted link. Not stored.", color = Tura.Muted, fontSize = 12.sp)
    }
}

@Composable
private fun MirrorBanner(pcName: String, onStop: () -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val control by dev.tunnelrat.TuraControlService.enabled.collectAsState()
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Tura.Surface).border(1.dp, Tura.AmberDim, RoundedCornerShape(16.dp)).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Showing this phone on $pcName", color = Tura.Text, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            TextButton(onClick = onStop) { Text("Stop", color = Tura.Amber) }
        }
        if (!control) {
            Text("To control it with the PC's mouse and keyboard, turn on “Tunnelrat: control from PC”.", color = Tura.Muted, fontSize = 12.sp)
            TextButton(onClick = {
                context.startActivity(android.content.Intent(android.provider.Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK))
            }) { Text("Open Accessibility settings", color = Tura.Amber) }
        }
    }
}

@Composable
private fun NetBanner(title: String, body: String) {
    Row(
        Modifier.fillMaxWidth().background(Tura.Danger.copy(alpha = 0.12f), CardShape)
            .border(1.dp, Tura.Danger.copy(alpha = 0.4f), CardShape).padding(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Rounded.WifiOff, contentDescription = null, tint = Tura.Danger)
        Spacer(Modifier.width(12.dp))
        Column {
            Text(title, color = Tura.Text, fontWeight = FontWeight.Bold, fontSize = 15.sp)
            Text(body, color = Tura.Muted, fontSize = 12.sp)
        }
    }
}

@Composable
private fun PairCard(modifier: Modifier, onScan: () -> Unit) {
    Column(
        modifier.background(Tura.Surface, CardShape).border(1.dp, Tura.Line, CardShape).padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(Icons.Rounded.QrCode2, contentDescription = null, tint = Tura.Amber, modifier = Modifier.size(72.dp))
        Text("Pair with your PC", color = Tura.Text, fontSize = 22.sp, fontWeight = FontWeight.Black)
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("On the PC, run  ", color = Tura.Muted, fontSize = 14.sp)
            Box(Modifier.clip(RoundedCornerShape(10.dp)).background(Tura.Bg).padding(horizontal = 12.dp, vertical = 6.dp)) {
                Text("tura pair", color = Tura.Amber, fontSize = 16.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace)
            }
        }
        Spacer(Modifier.height(8.dp))
        Row(
            Modifier.fillMaxWidth().height(60.dp).clip(RoundedCornerShape(20.dp)).background(Tura.Amber).clickable(onClick = onScan),
            horizontalArrangement = Arrangement.Center,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Rounded.QrCodeScanner, contentDescription = null, tint = Tura.Bg)
            Spacer(Modifier.width(10.dp))
            Text("Scan QR code", color = Tura.Bg, fontSize = 17.sp, fontWeight = FontWeight.Black)
        }
        Text("or scan it with your camera app and tap the link", color = Tura.Muted, fontSize = 12.sp, textAlign = TextAlign.Center)
    }
}

@Composable
private fun PowerRow(allowed: Set<PowerAction>?, onWake: () -> Unit, onPower: (PowerAction) -> Unit, onHint: (String) -> Unit) {
    Row(Modifier.fillMaxWidth().height(84.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        PowerTile("Wake", Icons.Rounded.WbSunny, accent = true, onFire = onWake)
        PowerTile("Lock", Icons.Rounded.Lock, onFire = { onPower(PowerAction.Lock) })
        PowerTile("Screen", Icons.Rounded.DesktopAccessDisabled, onFire = { onPower(PowerAction.ScreenOff) })
        // Only when the PC opted in: suspend can leave some GPUs dead.
        if (allowed?.contains(PowerAction.Suspend) == true) {
            PowerTile("Sleep", Icons.Rounded.Bedtime, onFire = { onPower(PowerAction.Suspend) })
        }
        PowerTile("Restart", Icons.Rounded.RestartAlt, hold = true, onEarlyRelease = { onHint("Hold Restart to restart the PC") }, onFire = { onPower(PowerAction.Reboot) })
        PowerTile("Off", Icons.Rounded.PowerSettingsNew, hold = true, danger = true, onEarlyRelease = { onHint("Hold Off to shut down the PC") }, onFire = { onPower(PowerAction.Shutdown) })
    }
}

/** `hold` tiles fire only after an 800 ms press so a stray tap can't kill the PC. */
@Composable
private fun RowScope.PowerTile(
    label: String,
    icon: ImageVector,
    accent: Boolean = false,
    danger: Boolean = false,
    hold: Boolean = false,
    onEarlyRelease: () -> Unit = {},
    onFire: () -> Unit,
) {
    val haptics = LocalHapticFeedback.current
    val scope = rememberCoroutineScope()
    var holding by remember { mutableStateOf(false) }
    val progress by animateFloatAsState(
        targetValue = if (holding) 1f else 0f,
        animationSpec = tween(if (holding) HOLD_MS.toInt() else 150),
        label = "hold",
    )
    val bg = if (accent) Tura.Amber else Tura.Surface
    val fg = when {
        accent -> Tura.Bg
        danger -> Tura.Danger
        else -> Tura.Text
    }
    val fill = if (danger) Tura.Danger.copy(alpha = 0.25f) else Tura.Amber.copy(alpha = 0.25f)

    Box(
        Modifier.weight(1f).fillMaxSize().clip(CardShape).background(bg)
            .border(1.dp, if (accent) Color.Transparent else Tura.Line, CardShape)
            .pointerInput(hold) {
                detectTapGestures(
                    onPress = {
                        if (!hold) {
                            if (tryAwaitRelease()) {
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                onFire()
                            }
                            return@detectTapGestures
                        }
                        holding = true
                        val job: Job = scope.launch {
                            delay(HOLD_MS)
                            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                            onFire()
                        }
                        tryAwaitRelease()
                        if (job.isActive) {
                            // Released before HOLD_MS: say why nothing happened.
                            job.cancel()
                            haptics.performHapticFeedback(HapticFeedbackType.Reject)
                            onEarlyRelease()
                        }
                        holding = false
                    },
                )
            },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.fillMaxSize().background(fill.copy(alpha = fill.alpha * progress)))
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(icon, contentDescription = label, tint = fg, modifier = Modifier.size(26.dp))
            Spacer(Modifier.height(4.dp))
            Text(if (hold && holding) "hold…" else label, color = fg, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

private const val HOLD_MS = 800L

@Composable
private fun MouseKey(label: String, modifier: Modifier, active: Boolean = false, onClick: () -> Unit) {
    val haptics = LocalHapticFeedback.current
    Box(
        modifier.fillMaxSize().clip(RoundedCornerShape(18.dp)).background(if (active) Tura.Amber else Tura.Surface)
            .border(1.dp, if (active) Color.Transparent else Tura.Line, RoundedCornerShape(18.dp))
            .clickable { haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove); onClick() },
        contentAlignment = Alignment.Center,
    ) { Text(label, color = if (active) Tura.Bg else Tura.Muted, fontWeight = FontWeight.Bold, fontSize = 18.sp) }
}
