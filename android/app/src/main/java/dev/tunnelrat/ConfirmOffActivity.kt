package dev.tunnelrat

import android.app.Activity
import android.app.AlertDialog
import android.os.Bundle
import android.widget.Toast
import dev.tunnelrat.proto.Message
import dev.tunnelrat.proto.PowerAction
import kotlinx.coroutines.launch

/** Dialog-only activity used by the widget's Off button: never shut down on a single stray tap. */
class ConfirmOffActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val t = tura
        val name = t.pc.value?.name ?: return finish()
        AlertDialog.Builder(this)
            .setTitle("Shut down $name?")
            .setMessage("Open apps on the PC get a chance to save.")
            .setPositiveButton("Shut down") { _, _ ->
                t.scope.launch {
                    val ok = t.sendWhenConnected(Message.Power(PowerAction.Shutdown))
                    Toast.makeText(applicationContext, if (ok) "Shutting down $name" else "$name isn't reachable", Toast.LENGTH_SHORT).show()
                }
                finish()
            }
            .setNegativeButton("Cancel") { _, _ -> finish() }
            .setOnCancelListener { finish() }
            .show()
    }
}
