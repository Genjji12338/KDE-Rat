package dev.tunnelrat.ui

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import androidx.activity.compose.BackHandler
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
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.ArrowForward
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Contrast
import androidx.compose.material.icons.rounded.Highlight
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay

/** evdev key codes used by presentation software. */
object SlideKeys {
    const val NEXT = 106 // Right — Impress, PowerPoint, Google Slides, PDF viewers
    const val PREVIOUS = 105 // Left
    const val START = 63 // F5
    const val END = 1 // Esc
}

/** Pointer speed: pixels per radian of phone rotation. */
private const val GYRO_GAIN = 900f

/**
 * Presenter remote: huge Next, smaller Previous, start/end, blank screen,
 * gyroscope "laser" pointer while held, and a timer. The screen stays on.
 * (Volume keys map to next/previous in MainActivity while this is shown.)
 */
@Composable
fun PresentationScreen(
    onKey: (Int) -> Unit,
    onBlank: () -> Unit,
    onPointer: (Float, Float) -> Unit,
    onClose: () -> Unit,
) {
    val haptics = LocalHapticFeedback.current
    BackHandler(onBack = onClose)

    // Keep the phone awake while presenting.
    val view = LocalView.current
    DisposableEffect(view) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }

    var startedAt by remember { mutableLongStateOf(System.currentTimeMillis()) }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            now = System.currentTimeMillis()
            delay(500)
        }
    }

    fun press(code: Int) {
        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
        onKey(code)
    }

    Column(
        Modifier.fillMaxSize().background(Tura.Bg).blockTouchesBelow().safeDrawingPadding().padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        // ── Top bar: close · timer · start/end ──
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(44.dp).clip(CircleShape).clickable(onClick = onClose), contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.Close, contentDescription = "Leave presentation mode", tint = Tura.Text)
            }
            Spacer(Modifier.weight(1f))
            val secs = (now - startedAt).coerceAtLeast(0) / 1000
            Text(
                "%d:%02d".format(secs / 60, secs % 60),
                color = Tura.Text,
                fontSize = 28.sp,
                fontWeight = FontWeight.Bold,
                fontFamily = FontFamily.Monospace,
                modifier = Modifier.clip(RoundedCornerShape(12.dp)).clickable { startedAt = System.currentTimeMillis() }.padding(horizontal = 10.dp, vertical = 2.dp),
            )
            Spacer(Modifier.weight(1f))
            Spacer(Modifier.size(44.dp))
        }
        Text("tap the timer to reset · volume keys: next / previous", color = Tura.Muted, fontSize = 12.sp, modifier = Modifier.align(Alignment.CenterHorizontally))

        Row(Modifier.fillMaxWidth().height(56.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SmallAction("Start", Icons.Rounded.PlayArrow) { press(SlideKeys.START); startedAt = System.currentTimeMillis() }
            SmallAction("End", Icons.Rounded.Stop) { press(SlideKeys.END) }
            SmallAction("Blank", Icons.Rounded.Contrast) { haptics.performHapticFeedback(HapticFeedbackType.LongPress); onBlank() }
        }

        // ── Previous (smaller) ──
        BigButton("Previous", Icons.AutoMirrored.Rounded.ArrowBack, accent = false, Modifier.fillMaxWidth().weight(0.32f)) {
            press(SlideKeys.PREVIOUS)
        }
        // ── Next (huge) ──
        BigButton("Next", Icons.AutoMirrored.Rounded.ArrowForward, accent = true, Modifier.fillMaxWidth().weight(0.68f)) {
            press(SlideKeys.NEXT)
        }

        PointerButton(onPointer, Modifier.fillMaxWidth().height(64.dp))
    }
}

@Composable
private fun BigButton(label: String, icon: ImageVector, accent: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val shape = RoundedCornerShape(28.dp)
    Box(
        modifier.clip(shape)
            .background(if (accent) Tura.Amber else Tura.Surface)
            .border(1.dp, if (accent) Color.Transparent else Tura.Line, shape)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(icon, contentDescription = null, tint = if (accent) Tura.Bg else Tura.Text, modifier = Modifier.size(if (accent) 64.dp else 40.dp))
            Text(label, color = if (accent) Tura.Bg else Tura.Text, fontSize = if (accent) 26.sp else 18.sp, fontWeight = FontWeight.Black)
        }
    }
}

@Composable
private fun RowScope.SmallAction(label: String, icon: ImageVector, onClick: () -> Unit) {
    val shape = RoundedCornerShape(18.dp)
    Row(
        Modifier.weight(1f).fillMaxSize().clip(shape).background(Tura.Surface).border(1.dp, Tura.Line, shape).clickable(onClick = onClick),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, contentDescription = null, tint = Tura.Text, modifier = Modifier.size(20.dp))
        Spacer(Modifier.width(6.dp))
        Text(label, color = Tura.Text, fontWeight = FontWeight.Bold)
    }
}

/** Hold to steer the PC's mouse pointer by moving the phone (gyroscope). */
@Composable
private fun PointerButton(onPointer: (Float, Float) -> Unit, modifier: Modifier) {
    val ctx = LocalContext.current
    val haptics = LocalHapticFeedback.current
    val sensors = remember { ctx.getSystemService(Context.SENSOR_SERVICE) as SensorManager }
    val gyro = remember { sensors.getDefaultSensor(Sensor.TYPE_GYROSCOPE) }
    val send by rememberUpdatedState(onPointer)
    var holding by remember { mutableStateOf(false) }

    DisposableEffect(holding) {
        if (!holding || gyro == null) return@DisposableEffect onDispose { }
        var last = 0L
        val listener = object : SensorEventListener {
            override fun onSensorChanged(e: SensorEvent) {
                if (last != 0L) {
                    val dt = (e.timestamp - last) / 1e9f
                    // Portrait phone pointed at the screen: turning left/right is
                    // rotation about the device's z axis, tilting up/down about x.
                    send(-e.values[2] * GYRO_GAIN * dt, -e.values[0] * GYRO_GAIN * dt)
                }
                last = e.timestamp
            }

            override fun onAccuracyChanged(s: Sensor?, a: Int) = Unit
        }
        sensors.registerListener(listener, gyro, SensorManager.SENSOR_DELAY_GAME)
        onDispose { sensors.unregisterListener(listener) }
    }

    val shape = RoundedCornerShape(20.dp)
    Row(
        modifier.clip(shape)
            .background(if (holding) Tura.Danger else Tura.SurfaceHigh)
            .border(1.dp, if (holding) Color.Transparent else Tura.Line, shape)
            .pointerInput(gyro) {
                detectTapGestures(onPress = {
                    if (gyro == null) return@detectTapGestures
                    holding = true
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    tryAwaitRelease()
                    holding = false
                })
            },
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Rounded.Highlight, contentDescription = null, tint = if (holding) Color.White else Tura.Text)
        Spacer(Modifier.width(8.dp))
        Text(
            when {
                gyro == null -> "No gyroscope on this phone"
                holding -> "Move the phone to point"
                else -> "Hold for pointer"
            },
            color = if (holding) Color.White else Tura.Text,
            fontWeight = FontWeight.Bold,
        )
    }
}

/**
 * Full-screen overlays must not let touches on their empty areas reach what's
 * underneath (the touchpad → stray PC clicks). Being a pointer-input node is
 * enough: hit testing stops at the topmost sibling that has one. It must NOT
 * consume, or the overlay's own tap detectors would see "consumed" and cancel.
 */
fun Modifier.blockTouchesBelow(): Modifier = pointerInput(Unit) {
    awaitPointerEventScope {
        while (true) awaitPointerEvent()
    }
}
