package dev.tunnelrat

import android.app.AlertDialog
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.widget.Toast
import dev.tunnelrat.proto.Message
import dev.tunnelrat.proto.PowerAction
import kotlinx.coroutines.launch

/** Quick-settings tile: wake the PC (Wake-on-LAN works even while the app is closed). */
class WakeTileService : TileService() {
    override fun onStartListening() {
        qsTile?.apply {
            state = if (tura.pc.value != null) Tile.STATE_INACTIVE else Tile.STATE_UNAVAILABLE
            subtitle = tura.pc.value?.name
            updateTile()
        }
    }

    override fun onClick() {
        tura.wake { msg -> Toast.makeText(applicationContext, msg, Toast.LENGTH_SHORT).show() }
    }
}

/** Quick-settings tile: shut the PC down, after a confirmation dialog. */
class OffTileService : TileService() {
    override fun onStartListening() {
        qsTile?.apply {
            state = if (tura.pc.value != null) Tile.STATE_INACTIVE else Tile.STATE_UNAVAILABLE
            subtitle = tura.pc.value?.name
            updateTile()
        }
    }

    override fun onClick() {
        val t = tura
        val name = t.pc.value?.name ?: return
        val dialog = AlertDialog.Builder(this)
            .setTitle("Shut down $name?")
            .setPositiveButton("Shut down") { _, _ ->
                t.scope.launch {
                    val ok = t.sendWhenConnected(Message.Power(PowerAction.Shutdown))
                    Toast.makeText(applicationContext, if (ok) "Shutting down $name" else "$name isn't reachable", Toast.LENGTH_SHORT).show()
                }
            }
            .setNegativeButton("Cancel", null)
            .create()
        showDialog(dialog)
    }
}
