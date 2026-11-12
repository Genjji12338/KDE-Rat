package dev.tunnelrat.ui

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.pm.ActivityInfo
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Keyboard
import androidx.compose.material.icons.rounded.Mouse
import androidx.compose.material.icons.rounded.TouchApp
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import dev.tunnelrat.TuraViewModel
import dev.tunnelrat.cast.VideoDecoder
import dev.tunnelrat.net.TuraClient
import dev.tunnelrat.proto.Message
import dev.tunnelrat.proto.MouseButton
import dev.tunnelrat.proto.PressState

private const val SCROLL_PX_PER_NOTCH = 40f

private tailrec fun Context.activity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.activity()
    else -> null
}

/**
 * The PC's screen, full-screen in landscape, controlled by touch (trackpad
 * or direct mode, see [TouchLayer]) and the phone keyboard.
 */
@Composable
fun CastScreen(vm: TuraViewModel, pcName: String, onClose: () -> Unit) {
    BackHandler(onBack = onClose)
    val context = LocalContext.current
    val view = LocalView.current

    // Landscape, screen on, no system bars while watching.
    DisposableEffect(Unit) {
        val activity = context.activity()
        val before = activity?.requestedOrientation
        activity?.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        view.keepScreenOn = true
        val bars = activity?.window?.let { WindowCompat.getInsetsController(it, view) }
        bars?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        bars?.hide(WindowInsetsCompat.Type.systemBars())
        onDispose {
            activity?.requestedOrientation = before ?: ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
            view.keepScreenOn = false
            bars?.show(WindowInsetsCompat.Type.systemBars())
        }
    }

    var video by remember { mutableStateOf<IntSize?>(null) }
    var status by remember { mutableStateOf<String?>(null) }
    var surface by remember { mutableStateOf<Surface?>(null) }
    var keyboard by remember { mutableStateOf(false) }
    var trackpad by remember { mutableStateOf(vm.tura.castTrackpad) }
    val state by vm.client.state.collectAsState()
    val connected = state is TuraClient.State.Connected

    // (Re)start the cast whenever we're connected.
    LaunchedEffect(connected) {
        if (connected) {
            val dm = context.resources.displayMetrics
            val long = maxOf(dm.widthPixels, dm.heightPixels)
            val short = minOf(dm.widthPixels, dm.heightPixels)
            status = "Asking $pcName…\nIf a dialog appears on the PC, choose a screen there."
            vm.client.send(Message.CastStart(long, short))
        } else {
            video = null
            status = "Reconnecting to $pcName…"
        }
    }
    LaunchedEffect(Unit) {
        vm.client.cast.collect { m ->
            when (m) {
                is Message.CastStarted -> {
                    video = IntSize(m.width, m.height)
                    status = null
                }
                is Message.CastEnded -> {
                    video = null
                    status = m.reason ?: "The PC stopped sharing its screen."
                }
                else -> Unit
            }
        }
    }
    DisposableEffect(Unit) { onDispose { vm.client.send(Message.CastStop) } }

    // A decoder for the current surface + stream size.
    val s = surface
    val v = video
    DisposableEffect(s, v) {
        if (s == null || v == null) return@DisposableEffect onDispose {}
        val decoder = VideoDecoder(s, v.width, v.height)
        vm.tura.videoSink = decoder::feed
        onDispose {
            vm.tura.videoSink = null
            decoder.release()
        }
    }

    Box(Modifier.fillMaxSize().background(Color.Black)) {
        if (v != null) {
            Box(Modifier.align(Alignment.Center).aspectRatio(v.width.toFloat() / v.height)) {
                AndroidView(
                    factory = { ctx ->
                        SurfaceView(ctx).apply {
                            holder.addCallback(
                                object : SurfaceHolder.Callback {
                                    override fun surfaceCreated(h: SurfaceHolder) { surface = h.surface }
                                    override fun surfaceChanged(h: SurfaceHolder, format: Int, width: Int, height: Int) {}
                                    override fun surfaceDestroyed(h: SurfaceHolder) { surface = null }
                                },
                            )
                        }
                    },
                    modifier = Modifier.fillMaxSize(),
                )
                TouchLayer(vm, trackpad, Modifier.fillMaxSize())
            }
        }

        status?.let {
            Text(
                it,
                color = Tura.Text,
                fontSize = 15.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.align(Alignment.Center).padding(32.dp),
            )
        }

        Column(Modifier.align(Alignment.TopEnd).padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            RoundButton(Icons.Rounded.Close, "Close the PC screen", onClose)
            RoundButton(Icons.Rounded.Keyboard, "Keyboard", active = keyboard) { keyboard = !keyboard }
            RoundButton(
                if (trackpad) Icons.Rounded.Mouse else Icons.Rounded.TouchApp,
                if (trackpad) "Trackpad mode (tap to switch to direct touch)" else "Direct touch (tap to switch to trackpad)",
            ) {
                trackpad = !trackpad
                vm.tura.castTrackpad = trackpad
            }
        }

        if (keyboard) {
            KeyboardBar(
                onText = { t, m -> vm.type(t, m) },
                onKey = { c, m -> vm.key(c, m) },
                modifier = Modifier.align(Alignment.BottomCenter).fillMaxWidth(0.7f).imePadding().padding(8.dp)
                    .clip(RoundedCornerShape(16.dp)).background(Tura.Surface.copy(alpha = 0.92f)),
            )
        }
    }
}

@Composable
private fun RoundButton(icon: ImageVector, label: String, onClick: () -> Unit) = RoundButton(icon, label, false, onClick)

@Composable
private fun RoundButton(icon: ImageVector, label: String, active: Boolean, onClick: () -> Unit) {
    Box(
        Modifier.size(44.dp).clip(CircleShape)
            .background(if (active) Tura.Amber.copy(alpha = 0.85f) else Color.Black.copy(alpha = 0.45f))
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = label, tint = if (active) Tura.Bg else Color.White)
    }
}

private const val POINTER_GAIN = 0.8f
private const val TAP_MAX_MS = 250L

/**
 * Touches on the video → the PC's pointer.
 *
 * Trackpad mode: drag moves the pointer from where it is, tap clicks at the
 * pointer. Direct mode: the pointer jumps to the finger, tap clicks there.
 * Both: long-press = right click, long-press then drag = left-button drag,
 * two fingers = scroll.
 */
@Composable
private fun TouchLayer(vm: TuraViewModel, trackpad: Boolean, modifier: Modifier) {
    val haptics = LocalHapticFeedback.current
    val hapticsNow by rememberUpdatedState(haptics)
    Box(
        modifier.pointerInput(trackpad) {
            fun at(p: Offset) = vm.pointerAt((p.x / size.width).coerceIn(0f, 1f), (p.y / size.height).coerceIn(0f, 1f))
            val slop = viewConfiguration.touchSlop
            val longPress = viewConfiguration.longPressTimeoutMillis
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false)
                down.consume()
                if (!trackpad) at(down.position)
                val start = down.position
                var last = down.position
                val t0 = down.uptimeMillis
                var moved = false
                var longPressed = false
                var dragging = false
                var multi = false
                var lastCentroid: Offset? = null
                while (true) {
                    val waitFor = if (moved || longPressed || multi) Long.MAX_VALUE else maxOf(1L, longPress - (android.os.SystemClock.uptimeMillis() - t0).coerceAtLeast(0))
                    val ev = if (waitFor == Long.MAX_VALUE) awaitPointerEvent() else withTimeoutOrNull(waitFor) { awaitPointerEvent() }
                    if (ev == null) {
                        // Held still: next is a right click (release) or a drag (move).
                        longPressed = true
                        hapticsNow.performHapticFeedback(HapticFeedbackType.LongPress)
                        continue
                    }
                    val pressed = ev.changes.filter { it.pressed }
                    ev.changes.forEach { it.consume() }
                    if (pressed.size >= 2) {
                        if (dragging) {
                            vm.button(MouseButton.Left, PressState.Up)
                            dragging = false
                        }
                        multi = true
                        val c = pressed.fold(Offset.Zero) { acc, p -> acc + p.position } / pressed.size.toFloat()
                        lastCentroid?.let { lc ->
                            val d = c - lc
                            vm.scroll(-d.x / SCROLL_PX_PER_NOTCH, -d.y / SCROLL_PX_PER_NOTCH)
                        }
                        lastCentroid = c
                        continue
                    }
                    lastCentroid = null
                    if (pressed.isEmpty()) {
                        val quick = (ev.changes.firstOrNull()?.uptimeMillis ?: Long.MAX_VALUE) - t0 < TAP_MAX_MS
                        when {
                            multi -> Unit
                            dragging -> vm.button(MouseButton.Left, PressState.Up)
                            longPressed && !moved -> vm.click(MouseButton.Right)
                            !moved && quick -> vm.click(MouseButton.Left)
                        }
                        break
                    }
                    if (multi) continue
                    val p = pressed.first().position
                    if (!moved && (p - start).getDistance() > slop) {
                        moved = true
                        last = start
                        if (longPressed) {
                            dragging = true
                            vm.button(MouseButton.Left, PressState.Down)
                        }
                    }
                    if (moved) {
                        if (trackpad) {
                            val d = p - last
                            vm.move(d.x * POINTER_GAIN, d.y * POINTER_GAIN)
                        } else {
                            at(p)
                        }
                    }
                    last = p
                }
            }
        },
    )
}
