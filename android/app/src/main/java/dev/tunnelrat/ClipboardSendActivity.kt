package dev.tunnelrat

import android.app.Activity
import android.widget.Toast

/**
 * Invisible activity behind the "Send clipboard" notification action.
 * Android only lets the *focused* app read the clipboard, so we take focus
 * for an instant, read, send, and close.
 */
class ClipboardSendActivity : Activity() {
    private var done = false

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        done = true
        // Launched from a notification, the shade can still own focus for a
        // moment and Android refuses the read; retry a few times briefly.
        val handler = android.os.Handler(mainLooper)
        fun attempt(left: Int) {
            val msg = tura.pushClipboardIfNew(force = true)
            if (msg == "Couldn't read the clipboard" && left > 0) {
                handler.postDelayed({ attempt(left - 1) }, 150)
                return
            }
            msg?.let { Toast.makeText(applicationContext, it, Toast.LENGTH_SHORT).show() }
            finish()
        }
        handler.postDelayed({ attempt(5) }, 100)
    }
}
