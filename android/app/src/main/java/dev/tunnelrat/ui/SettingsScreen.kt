package dev.tunnelrat.ui

import android.content.ComponentName
import android.content.Intent
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowBack
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import kotlin.math.roundToInt
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import dev.tunnelrat.Tura as TuraCore
import dev.tunnelrat.TuraNotificationListener
import dev.tunnelrat.TuraService

private val CardShape = RoundedCornerShape(24.dp)

@Composable
fun SettingsScreen(tura: TuraCore, onUnpair: () -> Unit, onClose: () -> Unit) {
    val ctx = LocalContext.current
    BackHandler(onBack = onClose)

    // Notification access is granted in system settings; re-check when we come back.
    fun hasAccess() = NotificationManagerCompat.getEnabledListenerPackages(ctx).contains(ctx.packageName)
    var access by remember { mutableStateOf(hasAccess()) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val obs = LifecycleEventObserver { _, e -> if (e == Lifecycle.Event.ON_RESUME) access = hasAccess() }
        lifecycle.addObserver(obs)
        onDispose { lifecycle.removeObserver(obs) }
    }

    val driveMode by tura.driveMode.collectAsState()
    val driveTree by tura.driveTree.collectAsState()
    var allFiles by remember { mutableStateOf(tura.hasAllFilesAccess()) }
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        uri?.let(tura::setDriveFolder)
    }
    DisposableEffect(lifecycle) {
        // Coming back from Android's "All files access" screen.
        val obs = LifecycleEventObserver { _, e ->
            if (e == Lifecycle.Event.ON_RESUME) {
                val now = tura.hasAllFilesAccess()
                if (now != allFiles) { allFiles = now; tura.refreshDrive() }
            }
        }
        lifecycle.addObserver(obs)
        onDispose { lifecycle.removeObserver(obs) }
    }

    val keep by tura.keepConnected.collectAsState()
    val clip by tura.syncClipboard.collectAsState()
    val notifs by tura.shareNotifications.collectAsState()
    val pc by tura.pc.collectAsState()

    Column(
        Modifier.fillMaxSize().background(Tura.Bg).blockTouchesBelow().safeDrawingPadding().padding(16.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(44.dp).clip(CircleShape).clickable(onClick = onClose), contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.ArrowBack, contentDescription = "Back", tint = Tura.Text)
            }
            Spacer(Modifier.width(8.dp))
            Text("Settings", color = Tura.Text, fontSize = 26.sp, fontWeight = FontWeight.Black)
        }

        Section {
            ToggleRow(
                "Stay connected in the background",
                "Needed for clipboard sync, notifications and battery. Shows a quiet status notification.",
                keep,
            ) {
                tura.set(tura.keepConnected, "keep_connected", it)
                TuraService.sync(ctx)
            }
            ToggleRow(
                "Sync clipboard",
                "PC → phone instantly. Phone → PC when you open Tunnelrat or tap “Send clipboard” in its notification (Android rule). Passwords marked sensitive are never sent.",
                clip,
            ) { tura.set(tura.syncClipboard, "sync_clipboard", it) }
            ToggleRow(
                "Phone notifications on the PC",
                if (access) "Shows your phone's notifications on the PC desktop." else "Needs notification access. Turning this on opens Android settings.",
                notifs && access,
            ) { on ->
                tura.set(tura.shareNotifications, "share_notifications", on)
                if (on && !access) {
                    val intent = if (android.os.Build.VERSION.SDK_INT >= 30) {
                        val component = ComponentName(ctx, TuraNotificationListener::class.java).flattenToString()
                        Intent(Settings.ACTION_NOTIFICATION_LISTENER_DETAIL_SETTINGS)
                            .putExtra(Settings.EXTRA_NOTIFICATION_LISTENER_COMPONENT_NAME, component)
                    } else {
                        Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS)
                    }
                    ctx.startActivity(intent)
                }
            }
        }

        Section {
            var speed by remember { mutableStateOf(tura.pointerSpeed) }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Pointer speed", color = Tura.Text, fontWeight = FontWeight.Bold, fontSize = 15.sp, modifier = Modifier.weight(1f))
                Text("%.1f×".format(speed), color = Tura.Muted, fontSize = 13.sp)
            }
            Text("Touchpad and the PC screen's trackpad mode", color = Tura.Muted, fontSize = 12.sp)
            Slider(
                value = speed,
                onValueChange = { speed = (it * 10).roundToInt() / 10f },
                onValueChangeFinished = { tura.pointerSpeed = speed },
                valueRange = dev.tunnelrat.MIN_POINTER_SPEED..dev.tunnelrat.MAX_POINTER_SPEED,
                colors = SliderDefaults.colors(thumbColor = Tura.Amber, activeTrackColor = Tura.Amber, inactiveTrackColor = Tura.Line),
            )
            if (speed != dev.tunnelrat.DEFAULT_POINTER_SPEED) {
                Text(
                    "Reset to ${dev.tunnelrat.DEFAULT_POINTER_SPEED}×",
                    color = Tura.Amber, fontSize = 13.sp,
                    modifier = Modifier.clickable { speed = dev.tunnelrat.DEFAULT_POINTER_SPEED; tura.pointerSpeed = speed },
                )
            }
        }

        if (pc != null) Section {
            val busy by tura.btSetupBusy.collectAsState()
            var paired by remember { mutableStateOf(tura.selectedBtPaired()) }
            LaunchedEffect(busy) { paired = tura.selectedBtPaired() }
            val addr = tura.selectedBtAddress
            Text("Bluetooth", color = Tura.Text, fontWeight = FontWeight.Bold, fontSize = 15.sp)
            Text(
                when {
                    addr == null -> "${pc!!.name} hasn't reported Bluetooth (none, or switched off)."
                    paired -> "Paired: without Wi-Fi, Tunnelrat reaches ${pc!!.name} over Bluetooth (everything but its screen). Keep Bluetooth on."
                    else -> "Pair once so Tunnelrat reaches ${pc!!.name} over Bluetooth when there's no Wi-Fi."
                },
                color = Tura.Muted, fontSize = 12.sp,
            )
            if (addr != null && !paired) {
                Text(
                    if (busy) "Pairing… confirm on the phone" else "Set up Bluetooth",
                    color = if (busy) Tura.Muted else Tura.Amber, fontWeight = FontWeight.Bold, fontSize = 14.sp,
                    modifier = Modifier.clickable(enabled = !busy) { tura.setUpBluetooth() }.padding(vertical = 6.dp),
                )
                val note by tura.btSetupNote.collectAsState()
                note?.let { Text(it, color = Tura.Danger, fontSize = 12.sp) }
            }
        }

        Section {
            Text("Files on the PC", color = Tura.Text, fontWeight = FontWeight.Bold, fontSize = 15.sp)
            Text(
                "Shows up in the PC's Files app as a drive while connected. Only your paired PC can see it.",
                color = Tura.Muted, fontSize = 12.sp,
            )
            ChoiceRow("Off", "Don't share files", driveMode == TuraCore.DriveMode.Off) {
                tura.setDriveMode(TuraCore.DriveMode.Off)
            }
            ChoiceRow(
                "A folder",
                driveTree?.let { "Sharing “${tura.folderName(it)}” · tap to change" } ?: "Pick one folder to share",
                driveMode == TuraCore.DriveMode.Folder,
            ) {
                if (driveTree == null || driveMode == TuraCore.DriveMode.Folder) pickFolder.launch(null)
                else tura.setDriveMode(TuraCore.DriveMode.Folder)
            }
            ChoiceRow(
                "Whole phone",
                if (allFiles) "All of your phone's storage" else "Needs “All files access” — opens Android settings",
                driveMode == TuraCore.DriveMode.All,
            ) {
                tura.setDriveMode(TuraCore.DriveMode.All)
                if (!allFiles) {
                    ctx.startActivity(
                        Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, android.net.Uri.fromParts("package", ctx.packageName, null)),
                    )
                }
            }
        }

        Section {
            Text("Also try", color = Tura.Muted, fontSize = 13.sp, fontWeight = FontWeight.Bold)
            Text("• Share sheet → “Send to PC”: links open on the PC, text goes to its clipboard, files to Downloads.", color = Tura.Text, fontSize = 14.sp)
            Text("• Quick settings → edit → add “Wake PC” and “PC off”.", color = Tura.Text, fontSize = 14.sp)
            Text("• Home screen → widgets → Tunnelrat.", color = Tura.Text, fontSize = 14.sp)
        }

        if (pc != null) {
            Section {
                Text("Paired with ${pc!!.name}", color = Tura.Text, fontWeight = FontWeight.Bold)
                Text("PC key ${pc!!.fingerprint.take(8)}…${pc!!.fingerprint.takeLast(4)}", color = Tura.Muted, fontSize = 13.sp)
                Box(
                    Modifier.clip(CircleShape).border(1.dp, Tura.Danger, CircleShape).clickable { onUnpair(); onClose() }
                        .padding(horizontal = 18.dp, vertical = 10.dp),
                ) { Text("Unpair", color = Tura.Danger, fontWeight = FontWeight.Bold) }
            }
        }
    }
}

@Composable
private fun Section(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().background(Tura.Surface, CardShape).border(1.dp, Tura.Line, CardShape).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) { content() }
}

@Composable
private fun ChoiceRow(title: String, subtitle: String, selected: Boolean, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).clickable(onClick = onClick).padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        RadioButton(
            selected = selected,
            onClick = onClick,
            colors = RadioButtonDefaults.colors(selectedColor = Tura.Amber, unselectedColor = Tura.Muted),
        )
        Spacer(Modifier.width(4.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = Tura.Text, fontWeight = FontWeight.Bold, fontSize = 15.sp)
            Text(subtitle, color = Tura.Muted, fontSize = 12.sp)
        }
    }
}

@Composable
private fun ToggleRow(title: String, subtitle: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().clickable { onChange(!checked) }, verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, color = Tura.Text, fontWeight = FontWeight.Bold, fontSize = 15.sp)
            Text(subtitle, color = Tura.Muted, fontSize = 12.sp)
        }
        Spacer(Modifier.width(12.dp))
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            colors = SwitchDefaults.colors(checkedTrackColor = Tura.Amber, checkedThumbColor = Tura.Bg),
        )
    }
}
