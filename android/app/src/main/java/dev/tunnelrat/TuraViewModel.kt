package dev.tunnelrat

import android.app.Application
import android.net.Uri
import androidx.lifecycle.AndroidViewModel
import dev.tunnelrat.proto.MediaAction
import dev.tunnelrat.proto.Message
import dev.tunnelrat.proto.Modifier
import dev.tunnelrat.proto.MouseButton
import dev.tunnelrat.proto.PowerAction
import dev.tunnelrat.proto.PressState

/** Thin UI façade over the app-wide [Tura] connection. */
class TuraViewModel(app: Application) : AndroidViewModel(app) {
    val tura = app.tura
    val client get() = tura.client
    val pc get() = tura.pc
    val offer get() = tura.offer
    val toast get() = tura.toast

    fun onPairLink(uri: Uri) = tura.onPairLink(uri)
    fun acceptOffer() = tura.acceptOffer()
    fun declineOffer() = tura.declineOffer()
    fun unpair() = tura.unpair()

    fun move(dx: Float, dy: Float) = client.send(Message.PointerMove(dx, dy))
    fun scroll(dx: Float, dy: Float) = client.send(Message.Scroll(dx, dy))
    fun click(b: MouseButton) = client.send(Message.PointerButton(b, PressState.Click))

    /** Three-finger swipes, like GNOME's own touchpad gestures. */
    fun threeFinger(direction: String) = when (direction) {
        "up" -> client.send(Message.Key(125, PressState.Click)) // Super: Activities overview
        "down" -> client.send(Message.Key(1, PressState.Click)) // Esc: leave the overview
        "left" -> client.send(Message.Key(109, PressState.Click, setOf(Modifier.Super))) // Super+PageDown: next workspace
        else -> client.send(Message.Key(104, PressState.Click, setOf(Modifier.Super))) // Super+PageUp: previous one
    }
    fun type(text: String, mods: Set<Modifier> = emptySet()) = if (text.isNotEmpty()) client.send(Message.Text(text, mods)) else true
    fun key(code: Int, mods: Set<Modifier> = emptySet()) = client.send(Message.Key(code, PressState.Click, mods))
    fun media(action: MediaAction) = client.send(Message.Media(action))
    fun volume(step: Int) = client.send(Message.VolumeStep(step))
    fun mute() = client.send(Message.MuteToggle)
    fun power(action: PowerAction) = tura.power(action)
    fun wake() = tura.wake()

    /** The PC's screen is shown full-screen. */
    val casting = kotlinx.coroutines.flow.MutableStateFlow(false)
    fun pointerAt(x: Float, y: Float) = client.send(Message.PointerAbs(x, y))
    fun button(b: MouseButton, s: PressState) = client.send(Message.PointerButton(b, s))

    /** Presentation mode: volume keys become next/previous slide. */
    val presenting = kotlinx.coroutines.flow.MutableStateFlow(false)

    fun toastShown() = tura.toastShown()
    fun notify(msg: String) = tura.notify(msg)
}
