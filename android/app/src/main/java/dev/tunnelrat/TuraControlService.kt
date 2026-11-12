package dev.tunnelrat

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.graphics.Path
import android.os.Bundle
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import dev.tunnelrat.proto.Message

/**
 * Carries out the PC's mouse and keyboard input while the phone's screen is
 * mirrored to it: gestures, back/home/recents and typing into the focused
 * field. Only acts on commands while mirroring (see [Tura.mirroring]).
 */
class TuraControlService : AccessibilityService() {
    companion object {
        @Volatile var instance: TuraControlService? = null
            private set

        /** A press released sooner than this (and without moving) is a tap. */
        private const val TAP_MS = 250L

        /** Whether the user turned the service on (for the UI hint). */
        val enabled = kotlinx.coroutines.flow.MutableStateFlow(false)
    }

    override fun onServiceConnected() {
        instance = this
        enabled.value = true
    }

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        instance = null
        enabled.value = false
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        instance = null
        enabled.value = false
        super.onDestroy()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}
    override fun onInterrupt() {}

    fun handle(m: Message) {
        when (m) {
            is Message.PhoneGesture -> gesture(m.path)
            is Message.PhoneTouch -> touch(m.action, m.x, m.y)
            is Message.PhoneNav -> performGlobalAction(
                when (m.action) {
                    "home" -> GLOBAL_ACTION_HOME
                    "recents" -> GLOBAL_ACTION_RECENTS
                    "notifications" -> GLOBAL_ACTION_NOTIFICATIONS
                    else -> GLOBAL_ACTION_BACK
                },
            )
            is Message.PhoneText -> type(m.text)
            else -> Unit
        }
    }

    // ── live touch: one continued stroke, extended segment by segment ──

    private var stroke: GestureDescription.StrokeDescription? = null
    private var last: android.graphics.PointF? = null
    private val queued = ArrayList<android.graphics.PointF>()
    private var lifting = false
    private var inFlight = false
    private var segmentStart = 0L

    private fun px(x: Float, y: Float): android.graphics.PointF {
        val (w, h) = dev.tunnelrat.cast.ScreenMirror.screenPixels(this)
        return android.graphics.PointF(x.coerceIn(0f, 1f) * (w - 1), y.coerceIn(0f, 1f) * (h - 1))
    }

    /** A press that's not yet a live touch: a quick release makes it a plain tap. */
    private var pressAt: android.graphics.PointF? = null
    private val main = android.os.Handler(android.os.Looper.getMainLooper())
    private val goLive = Runnable { startLive() }

    /** Runs on the main thread (callbacks too), so no locking. */
    private fun touch(action: String, x: Float, y: Float) {
        val p = px(x, y)
        when (action) {
            "down" -> {
                // Chained live segments add latency, so a click sent live would
                // last long enough to count as a long-press. Wait a moment first.
                main.removeCallbacks(goLive)
                queued.clear()
                lifting = false
                stroke = null
                pressAt = p
                main.postDelayed(goLive, TAP_MS)
            }
            "move" -> {
                val start = pressAt
                if (start != null) {
                    if (kotlin.math.hypot(p.x - start.x, p.y - start.y) <= slop) return
                    startLive()
                }
                if (stroke != null) {
                    queued += p
                    if (!inFlight) next()
                }
            }
            "up" -> {
                val start = pressAt
                if (start != null) {
                    main.removeCallbacks(goLive)
                    pressAt = null
                    dispatchGesture(
                        GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(segment(start, listOf(start)), 0, 40)).build(),
                        null, null,
                    )
                } else if (stroke != null) {
                    queued += p
                    lifting = true
                    if (!inFlight) next()
                }
            }
        }
    }

    /** Held or moved: from here on the finger follows the mouse. */
    private fun startLive() {
        main.removeCallbacks(goLive)
        val start = pressAt ?: return
        pressAt = null
        last = start
        send(GestureDescription.StrokeDescription(segment(start, listOf(start)), 0, 10, true))
    }

    private val slop by lazy { android.view.ViewConfiguration.get(this).scaledTouchSlop.toFloat() }

    private fun segment(from: android.graphics.PointF, to: List<android.graphics.PointF>) = Path().apply {
        moveTo(from.x, from.y)
        to.forEach { lineTo(it.x, it.y) } // a zero-length line still counts as a stroke
    }

    /** Extends the stroke with what arrived meanwhile (or holds still while the button is held). */
    private fun next() {
        val prev = stroke ?: return
        val from = last ?: return
        val points = if (queued.isEmpty()) listOf(from) else queued.toList()
        queued.clear()
        val idle = points.size == 1 && points[0] == from && !lifting
        val duration = if (idle) 50L else (android.os.SystemClock.uptimeMillis() - segmentStart).coerceIn(10L, 200L)
        last = points.last()
        val keepGoing = !lifting
        send(prev.continueStroke(segment(from, points), 0, duration, keepGoing))
        if (!keepGoing) {
            stroke = null
            last = null
            lifting = false
        }
    }

    private val touchCallback = object : GestureResultCallback() {
        override fun onCompleted(g: GestureDescription?) {
            inFlight = false
            if (stroke != null) next()
        }

        override fun onCancelled(g: GestureDescription?) {
            inFlight = false
            stroke = null
            last = null
            queued.clear()
        }
    }

    private fun send(s: GestureDescription.StrokeDescription) {
        stroke = if (s.willContinue()) s else null
        inFlight = true
        segmentStart = android.os.SystemClock.uptimeMillis()
        if (!dispatchGesture(GestureDescription.Builder().addStroke(s).build(), touchCallback, null)) {
            inFlight = false
            stroke = null
        }
    }

    private fun gesture(points: List<FloatArray>) {
        if (points.isEmpty()) return
        val (w, h) = dev.tunnelrat.cast.ScreenMirror.screenPixels(this)
        fun px(p: FloatArray) = p[0].coerceIn(0f, 1f) * (w - 1) to p[1].coerceIn(0f, 1f) * (h - 1)
        val path = Path()
        val (x0, y0) = px(points.first())
        path.moveTo(x0, y0)
        for (p in points.drop(1)) {
            val (x, y) = px(p)
            path.lineTo(x, y)
        }
        val duration = points.last()[2].toLong().coerceIn(40L, GestureDescription.getMaxGestureDuration())
        dispatchGesture(GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(path, 0, duration)).build(), null, null)
    }

    /** Types into the focused field: inserts at the cursor, `\n` = Enter, `\b` = Backspace. */
    private fun type(text: String) {
        val node = rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return
        if (!node.isEditable) return
        var current = if (node.isShowingHintText) "" else node.text?.toString() ?: ""
        var start = node.textSelectionStart.takeIf { it >= 0 } ?: current.length
        var end = node.textSelectionEnd.takeIf { it >= 0 } ?: start
        start = start.coerceIn(0, current.length)
        end = end.coerceIn(start, current.length)
        for (ch in text) {
            when (ch) {
                '\n' -> {
                    apply(node, current, start)
                    node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
                    return
                }
                '\b' -> {
                    if (start != end) {
                        current = current.removeRange(start, end)
                    } else if (start > 0) {
                        current = current.removeRange(start - 1, start)
                        start--
                    }
                    end = start
                }
                else -> {
                    current = current.substring(0, start) + ch + current.substring(end)
                    start++
                    end = start
                }
            }
        }
        apply(node, current, start)
    }

    private fun apply(node: AccessibilityNodeInfo, text: String, cursor: Int) {
        node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text) })
        node.performAction(
            AccessibilityNodeInfo.ACTION_SET_SELECTION,
            Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, cursor)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, cursor)
            },
        )
    }
}
