package dev.tunnelrat.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private const val TAP_SLOP_PX = 18f
private const val TAP_MAX_MS = 220L
private const val POINTER_GAIN = 1.6f
private const val SCROLL_PX_PER_NOTCH = 40f
/** Three fingers: a swipe fires after this much travel (once per gesture). */
private const val SWIPE_PX = 90f

/**
 * One finger: move. Tap: left click.
 * Two fingers: scroll. Two-finger tap: right click.
 * Three fingers: swipe up = overview, down = leave it, left/right = workspaces.
 * Three-finger tap: middle click.
 */
@Composable
fun Touchpad(
    onMove: (Float, Float) -> Unit,
    onScroll: (Float, Float) -> Unit,
    onLeftClick: () -> Unit,
    onRightClick: () -> Unit,
    modifier: Modifier = Modifier,
    onMiddleClick: () -> Unit = {},
    onSwipe: (String) -> Unit = {},
) {
    val haptics = LocalHapticFeedback.current
    var active by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(28.dp)

    Box(
        modifier
            .background(if (active) Tura.SurfaceHigh else Tura.Surface, shape)
            .border(1.dp, if (active) Tura.AmberDim else Tura.Line, shape)
            .pointerInput(Unit) {
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    active = true
                    val start = down.uptimeMillis
                    var maxFingers = 1
                    var travelled = 0f
                    var swipe = Offset.Zero
                    var swiped = false

                    while (true) {
                        val event = awaitPointerEvent()
                        val pressed = event.changes.filter { it.pressed }
                        if (pressed.isEmpty()) break
                        maxFingers = maxOf(maxFingers, pressed.size)

                        // Average movement of all pressed fingers.
                        val delta = pressed.fold(Offset.Zero) { acc, c -> acc + c.positionChange() } / pressed.size.toFloat()
                        travelled += delta.getDistance()
                        if (maxFingers >= 3) {
                            if (pressed.size >= 3 && !swiped) {
                                swipe += delta
                                if (swipe.getDistance() > SWIPE_PX) {
                                    swiped = true
                                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                    onSwipe(
                                        if (kotlin.math.abs(swipe.x) > kotlin.math.abs(swipe.y)) (if (swipe.x < 0) "left" else "right")
                                        else (if (swipe.y < 0) "up" else "down"),
                                    )
                                }
                            }
                        } else if (delta != Offset.Zero) {
                            if (pressed.size >= 2) {
                                onScroll(-delta.x / SCROLL_PX_PER_NOTCH, -delta.y / SCROLL_PX_PER_NOTCH)
                            } else if (maxFingers == 1) {
                                onMove(delta.x * POINTER_GAIN, delta.y * POINTER_GAIN)
                            }
                        }
                        event.changes.forEach { it.consume() }
                    }

                    val duration = currentEvent.changes.firstOrNull()?.uptimeMillis?.minus(start) ?: Long.MAX_VALUE
                    if (travelled < TAP_SLOP_PX && duration < TAP_MAX_MS) {
                        haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                        when {
                            maxFingers >= 3 -> onMiddleClick()
                            maxFingers == 2 -> onRightClick()
                            else -> onLeftClick()
                        }
                    }
                    active = false
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        if (!active) {
            Text(
                "drag · tap · two fingers to scroll\nthree fingers: ↑ overview · ←→ workspaces",
                color = Tura.Muted,
                fontSize = 13.sp,
                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            )
        }
    }
}
