package dev.tunnelrat.proto

import org.json.JSONObject

/**
 * Mirror of `pc/crates/tura-proto/src/lib.rs`. Keep both in sync:
 * one JSON object per WebSocket text frame, `type` selects the variant.
 */
const val PROTOCOL_VERSION = 1
const val DEFAULT_PORT = 47470
/** Raw bytes per FileChunk; matches `tura_proto::FILE_CHUNK`. */
const val FILE_CHUNK = 192 * 1024

enum class MouseButton(val wire: String) { Left("left"), Right("right"), Middle("middle") }
enum class PressState(val wire: String) { Down("down"), Up("up"), Click("click") }
enum class PowerAction(val wire: String) {
    Shutdown("shutdown"), Reboot("reboot"), Suspend("suspend"), Lock("lock"), ScreenOff("screen_off");

    companion object {
        fun fromWire(w: String) = entries.firstOrNull { it.wire == w }
    }
}

enum class Modifier(val wire: String) { Ctrl("ctrl"), Alt("alt"), Shift("shift"), Super("super") }

enum class MediaAction(val wire: String) { PlayPause("play_pause"), Next("next"), Previous("previous") }

data class NowPlaying(
    val player: String?,
    val title: String,
    val artist: String,
    val album: String,
    val playing: Boolean,
    /** `https://…` URL or `data:image/…;base64,…`. */
    val art: String?,
    val canNext: Boolean,
    val canPrevious: Boolean,
    val volume: Int,
    val muted: Boolean,
)

sealed interface Message {
    data class Hello(val deviceName: String, val deviceKind: String = "phone", val protocol: Int = PROTOCOL_VERSION) : Message
    /** [desktop] false = the PC is at its login screen. */
    data class PcInfo(val hostname: String, val wolMacs: List<String>, val powerActions: Set<PowerAction>, val desktop: Boolean = true) : Message
    /** Phone → PC at the login screen. */
    data class Login(val password: String) : Message
    data class LoginResult(val ok: Boolean, val message: String?) : Message
    data class Pair(val token: String, val deviceName: String) : Message
    data class PairOk(val pcName: String) : Message
    data class PointerMove(val dx: Float, val dy: Float) : Message
    data class PointerButton(val button: MouseButton, val state: PressState) : Message
    data class Scroll(val dx: Float, val dy: Float) : Message
    data class Key(val code: Int, val state: PressState, val modifiers: Set<Modifier> = emptySet()) : Message
    data class Text(val text: String, val modifiers: Set<Modifier> = emptySet()) : Message
    data class Power(val action: PowerAction) : Message
    data class Media(val action: MediaAction) : Message
    data class VolumeStep(val step: Int) : Message
    data object MuteToggle : Message
    data class NowPlayingMsg(val np: NowPlaying) : Message
    data class Clipboard(val text: String) : Message
    data class Battery(val level: Int, val charging: Boolean) : Message
    data class PhoneNotification(val key: String, val app: String, val title: String, val text: String, val canReply: Boolean = false) : Message
    /** PC → phone: answer notification [key] through its reply field. */
    data class PhoneReply(val key: String, val text: String) : Message
    data class PhoneNotificationRemoved(val key: String) : Message
    data class OpenUrl(val url: String) : Message
    data class FileStart(val id: Int, val name: String, val size: Long) : Message
    data class FileChunk(val id: Int, val data: String) : Message
    data class FileEnd(val id: Int) : Message
    data class FileSaved(val id: Int, val path: String) : Message
    /** Phone → PC: mount our files under [label], or unmount (null). */
    data class DriveOffer(val label: String?) : Message
    /** PC → phone: one file-system call; `op` is the raw JSON object. */
    data class FsRequest(val id: Long, val op: JSONObject) : Message
    data class FsReply(val id: Long, val result: JSONObject) : Message
    /** Phone → PC: stream your screen, scaled to fit. */
    data class CastStart(val maxWidth: Int, val maxHeight: Int) : Message
    data object CastStop : Message
    /** PC → phone: H.264 frames of this size follow (binary, kind 4). */
    data class CastStarted(val width: Int, val height: Int) : Message
    data class CastEnded(val reason: String?) : Message
    /** Pointer at (x, y) in the cast screen, each 0–1. */
    data class PointerAbs(val x: Float, val y: Float) : Message
    /** Phone → PC: our screen follows as binary VIDEO frames. */
    data class MirrorStart(val width: Int, val height: Int) : Message
    data object MirrorStop : Message
    /** PC → phone: please start mirroring. */
    data object MirrorRequest : Message
    /** PC → phone: touch gesture; each point is (x, y, t ms), x/y 0–1. */
    data class PhoneGesture(val path: List<FloatArray>) : Message
    /** PC → phone: live touch, [action] = "down" / "move" / "up", x/y 0–1. */
    data class PhoneTouch(val action: String, val x: Float, val y: Float) : Message
    /** PC → phone: back, home, recents, notifications. */
    data class PhoneNav(val action: String) : Message
    data class PhoneText(val text: String) : Message
    /** Ask the PC to drop this phone's pairing. */
    data object ForgetMe : Message
    data object Ping : Message
    data object Pong : Message
    data class Error(val message: String) : Message
    data class Unknown(val type: String) : Message

    fun toJson(): String = JSONObject().apply {
        when (val m = this@Message) {
            is Hello -> { put("type", "hello"); put("protocol", m.protocol); put("device_name", m.deviceName); put("device_kind", m.deviceKind) }
            is PcInfo -> { put("type", "pc_info"); put("hostname", m.hostname); put("wol_macs", m.wolMacs); put("power_actions", m.powerActions.map { it.wire }) }
            is Pair -> { put("type", "pair"); put("token", m.token); put("device_name", m.deviceName) }
            is PairOk -> { put("type", "pair_ok"); put("pc_name", m.pcName) }
            is PointerMove -> { put("type", "pointer_move"); put("dx", m.dx.toDouble()); put("dy", m.dy.toDouble()) }
            is PointerButton -> { put("type", "pointer_button"); put("button", m.button.wire); put("state", m.state.wire) }
            is Scroll -> { put("type", "scroll"); put("dx", m.dx.toDouble()); put("dy", m.dy.toDouble()) }
            is Key -> {
                put("type", "key"); put("code", m.code); put("state", m.state.wire)
                if (m.modifiers.isNotEmpty()) put("modifiers", org.json.JSONArray(m.modifiers.map { it.wire }))
            }
            is Text -> {
                put("type", "text"); put("text", m.text)
                if (m.modifiers.isNotEmpty()) put("modifiers", org.json.JSONArray(m.modifiers.map { it.wire }))
            }
            is Power -> { put("type", "power"); put("action", m.action.wire) }
            ForgetMe -> put("type", "forget_me")
            is DriveOffer -> { put("type", "drive_offer"); put("label", m.label ?: JSONObject.NULL) }
            is FsRequest -> { put("type", "fs_request"); put("id", m.id); put("op", m.op) }
            is FsReply -> { put("type", "fs_reply"); put("id", m.id); put("result", m.result) }
            is Clipboard -> { put("type", "clipboard"); put("text", m.text) }
            is Battery -> { put("type", "battery"); put("level", m.level); put("charging", m.charging) }
            is PhoneNotification -> { put("type", "phone_notification"); put("key", m.key); put("app", m.app); put("title", m.title); put("text", m.text); put("can_reply", m.canReply) }
            is PhoneReply -> put("type", "phone_reply") // PC → phone only
            is PhoneNotificationRemoved -> { put("type", "phone_notification_removed"); put("key", m.key) }
            is OpenUrl -> { put("type", "open_url"); put("url", m.url) }
            is FileStart -> { put("type", "file_start"); put("id", m.id); put("name", m.name); put("size", m.size) }
            is FileChunk -> { put("type", "file_chunk"); put("id", m.id); put("data", m.data) }
            is FileEnd -> { put("type", "file_end"); put("id", m.id) }
            is FileSaved -> { put("type", "file_saved"); put("id", m.id); put("path", m.path) }
            is CastStart -> { put("type", "cast_start"); put("max_width", m.maxWidth); put("max_height", m.maxHeight) }
            CastStop -> put("type", "cast_stop")
            is PointerAbs -> { put("type", "pointer_abs"); put("x", m.x.toDouble()); put("y", m.y.toDouble()) }
            is MirrorStart -> { put("type", "mirror_start"); put("width", m.width); put("height", m.height) }
            MirrorStop -> put("type", "mirror_stop")
            is Login -> { put("type", "login"); put("password", m.password) }
            is LoginResult -> put("type", "login_result")
            MirrorRequest -> put("type", "mirror_request")
            is PhoneGesture, is PhoneTouch, is PhoneNav, is PhoneText -> put("type", "unsupported") // PC → phone only
            is CastStarted -> put("type", "cast_started") // phone never sends these
            is CastEnded -> put("type", "cast_ended")
            is Media -> { put("type", "media"); put("action", m.action.wire) }
            is VolumeStep -> { put("type", "volume_step"); put("step", m.step) }
            MuteToggle -> put("type", "mute_toggle")
            is NowPlayingMsg -> put("type", "now_playing") // phone never sends this
            Ping -> put("type", "ping")
            Pong -> put("type", "pong")
            is Error -> { put("type", "error"); put("message", m.message) }
            is Unknown -> put("type", m.type)
        }
    }.toString()

    companion object {
        fun fromJson(s: String): Message {
            val o = JSONObject(s)
            return when (val type = o.optString("type")) {
                "hello" -> Hello(o.optString("device_name"), o.optString("device_kind"), o.optInt("protocol"))
                "pc_info" -> {
                    val macs = o.optJSONArray("wol_macs")
                    val power = o.optJSONArray("power_actions")
                    PcInfo(
                        hostname = o.optString("hostname"),
                        wolMacs = List(macs?.length() ?: 0) { macs!!.getString(it) },
                        powerActions = List(power?.length() ?: 0) { power!!.getString(it) }
                            .mapNotNull(PowerAction::fromWire).toSet(),
                        desktop = o.optBoolean("desktop", true),
                    )
                }
                "pair_ok" -> PairOk(o.optString("pc_name"))
                "clipboard" -> Clipboard(o.optString("text"))
                "fs_request" -> FsRequest(o.getLong("id"), o.getJSONObject("op"))
                "mirror_stop" -> MirrorStop
                "phone_reply" -> PhoneReply(o.optString("key"), o.optString("text"))
                "login_result" -> LoginResult(o.optBoolean("ok"), o.optString("message").takeUnless { o.isNull("message") || it.isEmpty() })
                "mirror_request" -> MirrorRequest
                "phone_gesture" -> {
                    val a = o.getJSONArray("path")
                    PhoneGesture(List(a.length()) { i -> a.getJSONArray(i).let { p -> FloatArray(3) { p.getDouble(it).toFloat() } } })
                }
                "phone_touch" -> PhoneTouch(o.optString("action"), o.optDouble("x").toFloat(), o.optDouble("y").toFloat())
                "phone_nav" -> PhoneNav(o.optString("action"))
                "phone_text" -> PhoneText(o.optString("text"))
                "cast_started" -> CastStarted(o.optInt("width"), o.optInt("height"))
                "cast_ended" -> CastEnded(o.optString("reason").takeUnless { o.isNull("reason") || it.isEmpty() })
                "file_saved" -> FileSaved(o.optInt("id"), o.optString("path"))
                "now_playing" -> NowPlayingMsg(
                    NowPlaying(
                        player = o.optString("player").takeUnless { o.isNull("player") || it.isEmpty() },
                        title = o.optString("title"),
                        artist = o.optString("artist"),
                        album = o.optString("album"),
                        playing = o.optBoolean("playing"),
                        art = o.optString("art").takeUnless { o.isNull("art") || it.isEmpty() },
                        canNext = o.optBoolean("can_next", true),
                        canPrevious = o.optBoolean("can_previous", true),
                        volume = o.optInt("volume"),
                        muted = o.optBoolean("muted"),
                    ),
                )
                "ping" -> Ping
                "pong" -> Pong
                "error" -> Error(o.optString("message"))
                else -> Unknown(type)
            }
        }
    }
}
