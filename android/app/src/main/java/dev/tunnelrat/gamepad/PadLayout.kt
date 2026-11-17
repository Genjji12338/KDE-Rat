package dev.tunnelrat.gamepad

import org.json.JSONObject

/** Button bits, same as turad's `tura_proto::pad`. */
object Pad {
    const val A = 1 shl 0
    const val B = 1 shl 1
    const val X = 1 shl 2
    const val Y = 1 shl 3
    const val LB = 1 shl 4
    const val RB = 1 shl 5
    const val VIEW = 1 shl 6
    const val MENU = 1 shl 7
    const val HOME = 1 shl 8
    const val L3 = 1 shl 9
    const val R3 = 1 shl 10
    const val UP = 1 shl 11
    const val DOWN = 1 shl 12
    const val LEFT = 1 shl 13
    const val RIGHT = 1 shl 14
}

enum class Kind { Stick, Dpad, Face, Shoulder, Trigger, Small, Round }

/**
 * Every movable control: its look ([kind]), size at scale 1 in dp, and
 * default centre as a fraction of the (landscape) screen.
 */
enum class PadControl(val label: String, val kind: Kind, val w: Int, val h: Int, val bit: Int, val x: Float, val y: Float) {
    LStick("Left stick", Kind.Stick, 160, 160, Pad.L3, 0.16f, 0.60f),
    RStick("Right stick", Kind.Stick, 130, 130, Pad.R3, 0.63f, 0.76f),
    Dpad("D-pad", Kind.Dpad, 130, 130, 0, 0.37f, 0.76f),
    A("A", Kind.Face, 76, 76, Pad.A, 0.85f, 0.75f),
    B("B", Kind.Face, 64, 64, Pad.B, 0.93f, 0.55f),
    X("X", Kind.Face, 64, 64, Pad.X, 0.77f, 0.55f),
    Y("Y", Kind.Face, 64, 64, Pad.Y, 0.85f, 0.37f),
    LB("LB", Kind.Shoulder, 130, 44, Pad.LB, 0.11f, 0.27f),
    RB("RB", Kind.Shoulder, 130, 44, Pad.RB, 0.89f, 0.17f),
    LT("LT", Kind.Trigger, 130, 52, 0, 0.11f, 0.12f),
    RT("RT", Kind.Trigger, 130, 52, 0, 0.66f, 0.17f),
    View("View", Kind.Small, 64, 34, Pad.VIEW, 0.42f, 0.42f),
    Home("Home", Kind.Round, 42, 42, Pad.HOME, 0.50f, 0.42f),
    Menu("Menu", Kind.Small, 64, 34, Pad.MENU, 0.58f, 0.42f),
}

/** Where a control sits: centre (fractions of the screen) and size factor. */
data class Placement(val x: Float, val y: Float, val scale: Float = 1f)

/** The user's arrangement; missing controls use their defaults. */
data class PadLayout(val places: Map<PadControl, Placement>) {
    operator fun get(c: PadControl) = places[c] ?: Placement(c.x, c.y)

    fun with(c: PadControl, p: Placement) = PadLayout(places + (c to p))

    fun toJson(): String = JSONObject().apply {
        for ((c, p) in places) put(c.name, JSONObject().put("x", p.x.toDouble()).put("y", p.y.toDouble()).put("s", p.scale.toDouble()))
    }.toString()

    companion object {
        val Default = PadLayout(emptyMap())
        const val MIN_SCALE = 0.6f
        const val MAX_SCALE = 1.8f

        fun fromJson(s: String?): PadLayout {
            val o = runCatching { JSONObject(s ?: return Default) }.getOrNull() ?: return Default
            val places = PadControl.entries.mapNotNull { c ->
                val p = o.optJSONObject(c.name) ?: return@mapNotNull null
                c to Placement(
                    p.optDouble("x", c.x.toDouble()).toFloat().coerceIn(0f, 1f),
                    p.optDouble("y", c.y.toDouble()).toFloat().coerceIn(0f, 1f),
                    p.optDouble("s", 1.0).toFloat().coerceIn(MIN_SCALE, MAX_SCALE),
                )
            }.toMap()
            return PadLayout(places)
        }
    }
}

/**
 * The controller's state, changed from several touch handlers at once;
 * [dirty] tells the sender loop something changed.
 */
class PadState {
    @Volatile var buttons = 0
        private set
    @Volatile var lx = 0f
    @Volatile var ly = 0f
    @Volatile var rx = 0f
    @Volatile var ry = 0f
    @Volatile var lt = 0f
    @Volatile var rt = 0f
    @Volatile var dirty = true

    @Synchronized fun press(bit: Int, down: Boolean) {
        val b = if (down) buttons or bit else buttons and bit.inv()
        if (b != buttons) {
            buttons = b
            dirty = true
        }
    }

    /** Replaces the d-pad bits. */
    @Synchronized fun dpad(bits: Int) {
        val mask = Pad.UP or Pad.DOWN or Pad.LEFT or Pad.RIGHT
        val b = (buttons and mask.inv()) or bits
        if (b != buttons) {
            buttons = b
            dirty = true
        }
    }

    fun stick(left: Boolean, x: Float, y: Float) {
        if (left) { lx = x; ly = y } else { rx = x; ry = y }
        dirty = true
    }

    fun trigger(left: Boolean, v: Float) {
        if (left) lt = v else rt = v
        dirty = true
    }

    @Synchronized fun reset() {
        buttons = 0; lx = 0f; ly = 0f; rx = 0f; ry = 0f; lt = 0f; rt = 0f
        dirty = true
    }
}
