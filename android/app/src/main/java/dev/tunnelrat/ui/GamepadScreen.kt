package dev.tunnelrat.ui

import android.content.Context
import android.content.pm.ActivityInfo
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.view.Surface
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Edit
import androidx.compose.material.icons.rounded.Home
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.KeyboardArrowLeft
import androidx.compose.material.icons.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.KeyboardArrowUp
import androidx.compose.material.icons.rounded.ScreenRotation
import androidx.compose.material3.Icon
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import dev.tunnelrat.TuraViewModel
import dev.tunnelrat.gamepad.Kind
import dev.tunnelrat.gamepad.Pad
import dev.tunnelrat.gamepad.PadControl
import dev.tunnelrat.gamepad.PadLayout
import dev.tunnelrat.gamepad.PadState
import dev.tunnelrat.gamepad.Placement
import dev.tunnelrat.net.TuraClient
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.roundToInt

/** Right stick deflection per rad/s of phone rotation, with gyro aiming on. */
private const val GYRO_GAIN = 0.55f
/** A stick released this quickly without moving is a click (L3 / R3). */
private const val STICK_TAP_MS = 180L

/**
 * Gamepad mode: the phone is a controller for the PC (a virtual Xbox pad
 * there). Landscape, screen on, every control movable in the editor.
 * Volume keys are LB / RB while this is open (MainActivity).
 */
@Composable
fun GamepadScreen(vm: TuraViewModel, pcName: String, onClose: () -> Unit) {
    BackHandler(onBack = onClose)
    val context = LocalContext.current
    val view = LocalView.current
    val pad = vm.pad
    var layout by remember { mutableStateOf(vm.tura.padLayout) }
    var editing by remember { mutableStateOf(false) }
    var gyro by remember { mutableStateOf(vm.tura.padGyro) }
    val state by vm.client.state.collectAsState()

    // Landscape, screen on, no system bars.
    DisposableEffect(Unit) {
        val activity = context.activity()
        val before = activity?.requestedOrientation
        activity?.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        view.keepScreenOn = true
        val bars = activity?.window?.let { WindowCompat.getInsetsController(it, view) }
        bars?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        bars?.hide(WindowInsetsCompat.Type.systemBars())
        onDispose {
            pad.reset()
            vm.sendPad()
            vm.endPad()
            activity?.requestedOrientation = before ?: ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
            view.keepScreenOn = false
            bars?.show(WindowInsetsCompat.Type.systemBars())
        }
    }

    // Send the state when it changed, at most every 8 ms (and again after a
    // reconnect, so the PC's controller is never stuck with a stale state).
    LaunchedEffect(state is TuraClient.State.Connected) {
        pad.dirty = true
        while (true) {
            if (pad.dirty) {
                pad.dirty = false
                if (!vm.sendPad()) pad.dirty = true
            }
            delay(8)
        }
    }

    if (gyro && !editing) GyroAim(pad)

    if (editing) {
        PadEditor(
            initial = layout,
            onDone = {
                layout = it
                vm.tura.padLayout = it
                editing = false
            },
            onCancel = { editing = false },
        )
        return
    }

    Box(Modifier.fillMaxSize().background(Tura.Bg).blockTouchesBelow()) {
        PadSurface(layout) { c, size -> PlayControl(c, size, pad) }

        // Top bar: close · status · edit · gyro. Fixed, not movable.
        Row(
            Modifier.align(Alignment.TopCenter).padding(top = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            TopButton(Icons.Rounded.Close, "Leave gamepad mode", false, onClose)
            Text(
                if (state is TuraClient.State.Connected) "Controller · $pcName" else "$pcName · reconnecting…",
                color = if (state is TuraClient.State.Connected) Tura.Text else Tura.Amber,
                fontSize = 13.sp,
                fontWeight = FontWeight.Bold,
                modifier = Modifier.widthIn(max = 200.dp),
                maxLines = 1,
            )
            TopButton(Icons.Rounded.Edit, "Move buttons", false) {
                pad.reset()
                editing = true
            }
            TopButton(Icons.Rounded.ScreenRotation, if (gyro) "Gyro aiming on" else "Gyro aiming off", gyro) {
                gyro = !gyro
                vm.tura.padGyro = gyro
                if (!gyro) pad.stick(false, 0f, 0f)
            }
        }
    }
}

/** Lays the controls out at their placements on the full screen. */
@Composable
private fun PadSurface(layout: PadLayout, content: @Composable (PadControl, Placement) -> Unit) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val density = LocalDensity.current
        val w = constraints.maxWidth
        val h = constraints.maxHeight
        for (c in PadControl.entries) {
            val p = layout[c]
            val cw = with(density) { (c.w * p.scale).dp.roundToPx() }
            val ch = with(density) { (c.h * p.scale).dp.roundToPx() }
            Box(
                Modifier
                    .offset { IntOffset((p.x * w - cw / 2f).roundToInt(), (p.y * h - ch / 2f).roundToInt()) }
                    .size(with(density) { cw.toDp() }, with(density) { ch.toDp() }),
            ) { content(c, p) }
        }
    }
}

private fun shapeOf(c: PadControl): Shape = when (c.kind) {
    Kind.Face, Kind.Round, Kind.Stick -> CircleShape
    Kind.Small -> RoundedCornerShape(50)
    else -> RoundedCornerShape(14.dp)
}

/** How a control looks, pressed or not; [knob] is the stick's deflection. */
@Composable
private fun ControlLook(c: PadControl, pressed: Boolean, knob: Offset = Offset.Zero, dpad: Int = 0, outline: Color? = null) {
    val shape = shapeOf(c)
    val accent = c == PadControl.A
    val bg = when {
        accent && pressed -> Tura.AmberDim
        accent -> Tura.Amber
        pressed -> Tura.SurfaceHigh
        else -> Tura.Surface
    }
    when (c.kind) {
        Kind.Dpad -> Box(Modifier.fillMaxSize()) {
            val arm = Modifier.fillMaxSize(0.36f)
            for ((bit, align, icon) in listOf(
                Triple(Pad.UP, Alignment.TopCenter, Icons.Rounded.KeyboardArrowUp),
                Triple(Pad.DOWN, Alignment.BottomCenter, Icons.Rounded.KeyboardArrowDown),
                Triple(Pad.LEFT, Alignment.CenterStart, Icons.Rounded.KeyboardArrowLeft),
                Triple(Pad.RIGHT, Alignment.CenterEnd, Icons.Rounded.KeyboardArrowRight),
            )) {
                val on = dpad and bit != 0
                Box(
                    arm.align(align).clip(RoundedCornerShape(10.dp))
                        .background(if (on) Tura.SurfaceHigh else Tura.Surface)
                        .border(1.dp, outline ?: if (on) Tura.AmberDim else Tura.Line, RoundedCornerShape(10.dp)),
                    contentAlignment = Alignment.Center,
                ) { Icon(icon, contentDescription = null, tint = Tura.Text) }
            }
        }
        Kind.Stick -> Box(
            Modifier.fillMaxSize().clip(shape).background(Tura.Surface).border(1.dp, outline ?: Tura.Line, shape),
            contentAlignment = Alignment.Center,
        ) {
            BoxWithConstraints(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                val travel = maxWidth * 0.29f
                Box(
                    Modifier.offset(travel * knob.x, travel * knob.y).size(maxWidth * 0.42f).clip(CircleShape)
                        .background(if (pressed) Tura.SurfaceHigh else Tura.Line)
                        .border(1.dp, if (pressed) Tura.Amber else Tura.AmberDim, CircleShape),
                )
            }
        }
        else -> Box(
            Modifier.fillMaxSize().clip(shape).background(bg)
                .border(1.dp, outline ?: if (accent) Color.Transparent else if (pressed) Tura.AmberDim else Tura.Line, shape),
            contentAlignment = Alignment.Center,
        ) {
            if (c == PadControl.Home) {
                Icon(Icons.Rounded.Home, contentDescription = "Home", tint = Tura.Text, modifier = Modifier.size(20.dp))
            } else {
                Text(
                    c.label,
                    color = if (accent) Tura.Bg else Tura.Text,
                    fontWeight = FontWeight.Black,
                    fontSize = when (c.kind) {
                        Kind.Face -> if (accent) 26.sp else 22.sp
                        Kind.Small -> 12.sp
                        else -> 15.sp
                    },
                )
            }
        }
    }
}

/** One control in play mode: its own finger, independent of the others. */
@Composable
private fun PlayControl(c: PadControl, place: Placement, pad: PadState) {
    val haptics = LocalHapticFeedback.current
    val scope = rememberCoroutineScope()
    var pressed by remember { mutableStateOf(false) }
    var knob by remember { mutableStateOf(Offset.Zero) }
    var dpadBits by remember { mutableStateOf(0) }

    val input = Modifier.fillMaxSize().pointerInput(c, place) {
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false)
            down.consume()
            val id = down.id
            pressed = true
            haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
            val centre = Offset(size.width / 2f, size.height / 2f)
            // The left stick floats: it centres where the thumb lands.
            val origin = if (c == PadControl.LStick) down.position else centre
            val radius = size.width * 0.38f
            var travelled = 0f

            fun stickAt(p: Offset) {
                var v = (p - origin) / radius
                val len = v.getDistance()
                if (len > 1f) v /= len
                knob = v
                pad.stick(c == PadControl.LStick, v.x, v.y)
            }

            fun dpadAt(p: Offset) {
                val d = p - centre
                val dead = size.width * 0.12f
                var bits = 0
                // 8-way: a direction counts when it's at least ~40% of the other.
                if (abs(d.x) > dead && abs(d.x) > abs(d.y) * 0.4f) bits = bits or if (d.x < 0) Pad.LEFT else Pad.RIGHT
                if (abs(d.y) > dead && abs(d.y) > abs(d.x) * 0.4f) bits = bits or if (d.y < 0) Pad.UP else Pad.DOWN
                if (bits != dpadBits && bits != 0) haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                dpadBits = bits
                pad.dpad(bits)
            }

            when (c.kind) {
                Kind.Stick -> stickAt(down.position)
                Kind.Dpad -> dpadAt(down.position)
                Kind.Trigger -> pad.trigger(c == PadControl.LT, 1f)
                else -> pad.press(c.bit, true)
            }
            while (true) {
                val ev = awaitPointerEvent()
                val mine = ev.changes.firstOrNull { it.id == id } ?: continue
                mine.consume()
                if (!mine.pressed) break
                travelled += (mine.position - mine.previousPosition).getDistance()
                when (c.kind) {
                    Kind.Stick -> stickAt(mine.position)
                    Kind.Dpad -> dpadAt(mine.position)
                    else -> Unit
                }
            }
            pressed = false
            when (c.kind) {
                Kind.Stick -> {
                    knob = Offset.Zero
                    pad.stick(c == PadControl.LStick, 0f, 0f)
                    // Quick tap on a stick: click it (L3 / R3).
                    val quick = (currentEvent.changes.firstOrNull()?.uptimeMillis ?: Long.MAX_VALUE) - down.uptimeMillis < STICK_TAP_MS
                    if (quick && travelled < viewConfiguration.touchSlop) {
                        pad.press(c.bit, true)
                        scope.launch {
                            delay(60)
                            pad.press(c.bit, false)
                        }
                    }
                }
                Kind.Dpad -> {
                    dpadBits = 0
                    pad.dpad(0)
                }
                Kind.Trigger -> pad.trigger(c == PadControl.LT, 0f)
                else -> pad.press(c.bit, false)
            }
        }
    }
    Box(input) { ControlLook(c, pressed, knob, dpadBits) }
}

/** Gyro aiming: turning/tilting the phone steers the right stick. */
@Composable
private fun GyroAim(pad: PadState) {
    val context = LocalContext.current
    val view = LocalView.current
    DisposableEffect(Unit) {
        val sensors = context.getSystemService(Context.SENSOR_SERVICE) as SensorManager
        val gyro = sensors.getDefaultSensor(Sensor.TYPE_GYROSCOPE) ?: return@DisposableEffect onDispose { }
        val listener = object : SensorEventListener {
            override fun onSensorChanged(e: SensorEvent) {
                // Landscape: turning left/right is rotation about the device's
                // x axis, tilting up/down about its y axis; flipped for the
                // other landscape side.
                val sign = if (view.display?.rotation == Surface.ROTATION_270) -1f else 1f
                val x = (-e.values[0] * GYRO_GAIN * sign).coerceIn(-1f, 1f)
                val y = (-e.values[1] * GYRO_GAIN * sign).coerceIn(-1f, 1f)
                pad.stick(false, if (abs(x) < 0.04f) 0f else x, if (abs(y) < 0.04f) 0f else y)
            }

            override fun onAccuracyChanged(s: Sensor?, a: Int) = Unit
        }
        sensors.registerListener(listener, gyro, SensorManager.SENSOR_DELAY_GAME)
        onDispose {
            sensors.unregisterListener(listener)
            pad.stick(false, 0f, 0f)
        }
    }
}

/**
 * The layout editor: drag controls anywhere, tap one to resize it with the
 * slider. Done saves, × throws the changes away.
 */
@Composable
private fun PadEditor(initial: PadLayout, onDone: (PadLayout) -> Unit, onCancel: () -> Unit) {
    BackHandler(onBack = onCancel)
    var layout by remember { mutableStateOf(initial) }
    var selected by remember { mutableStateOf<PadControl?>(null) }

    Box(Modifier.fillMaxSize().background(Tura.Bg).blockTouchesBelow()) {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            val w = constraints.maxWidth.toFloat()
            val h = constraints.maxHeight.toFloat()
            PadSurface(layout) { c, place ->
                Box(
                    Modifier.fillMaxSize()
                        .pointerInput(c) {
                            detectTapGestures(onPress = { selected = c })
                        }
                        .pointerInput(c, w, h) {
                            detectDragGestures(onDragStart = { selected = c }) { change, drag ->
                                change.consume()
                                val p = layout[c]
                                layout = layout.with(c, p.copy(x = (p.x + drag.x / w).coerceIn(0.02f, 0.98f), y = (p.y + drag.y / h).coerceIn(0.02f, 0.98f)))
                            }
                        },
                ) { ControlLook(c, pressed = false, outline = if (c == selected) Tura.Amber else Tura.Line) }
            }
        }

        // Editor bar.
        Row(
            Modifier.align(Alignment.TopCenter).padding(top = 10.dp).clip(RoundedCornerShape(18.dp))
                .background(Tura.Surface.copy(alpha = 0.95f)).border(1.dp, Tura.Line, RoundedCornerShape(18.dp))
                .padding(horizontal = 10.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            TopButton(Icons.Rounded.Close, "Discard changes", false, onCancel)
            val sel = selected
            if (sel == null) {
                Text("Drag buttons to move them · tap one to resize", color = Tura.Muted, fontSize = 13.sp)
            } else {
                Text(sel.label, color = Tura.Text, fontWeight = FontWeight.Bold, fontSize = 13.sp, modifier = Modifier.widthIn(min = 72.dp))
                Slider(
                    value = layout[sel].scale,
                    onValueChange = { layout = layout.with(sel, layout[sel].copy(scale = it)) },
                    valueRange = PadLayout.MIN_SCALE..PadLayout.MAX_SCALE,
                    colors = SliderDefaults.colors(thumbColor = Tura.Amber, activeTrackColor = Tura.Amber, inactiveTrackColor = Tura.Line),
                    modifier = Modifier.width(180.dp).height(32.dp),
                )
            }
            Pill("Reset") {
                layout = PadLayout.Default
                selected = null
            }
            Pill("Done", accent = true) { onDone(layout) }
        }
    }
}

@Composable
private fun TopButton(icon: ImageVector, label: String, active: Boolean, onClick: () -> Unit) {
    Box(
        Modifier.size(40.dp).clip(CircleShape)
            .background(if (active) Tura.Amber else Tura.Surface)
            .border(1.dp, if (active) Color.Transparent else Tura.Line, CircleShape)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Icon(icon, contentDescription = label, tint = if (active) Tura.Bg else Tura.Text, modifier = Modifier.size(20.dp)) }
}

@Composable
private fun Pill(label: String, accent: Boolean = false, onClick: () -> Unit) {
    Box(
        Modifier.clip(RoundedCornerShape(50)).background(if (accent) Tura.Amber else Tura.SurfaceHigh)
            .clickable(onClick = onClick).padding(horizontal = 14.dp, vertical = 8.dp),
    ) { Text(label, color = if (accent) Tura.Bg else Tura.Text, fontWeight = FontWeight.Bold, fontSize = 13.sp) }
}
