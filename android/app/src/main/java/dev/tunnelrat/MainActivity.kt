package dev.tunnelrat

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.KeyEvent
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import dev.tunnelrat.ui.HomeScreen
import dev.tunnelrat.ui.TunnelratTheme

class MainActivity : ComponentActivity() {
    companion object {
        /** Opened by the PC's mirror request: ask to capture the screen right away. */
        const val ACTION_MIRROR = "dev.tunnelrat.ACTION_MIRROR"
    }

    private val vm: TuraViewModel by viewModels()

    /**
     * ACCESS_LOCAL_NETWORK: Android 17 blocks LAN traffic (silently: connects
     * just time out) without it. POST_NOTIFICATIONS: the background-connection
     * status notification. BLUETOOTH_CONNECT: reaching the PC over Bluetooth
     * when the network can't.
     */
    private val askPermissions = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { res ->
        if (res[Manifest.permission.ACCESS_LOCAL_NETWORK] == false) vm.notify("Tunnelrat needs “Nearby devices” access to reach your PC")
        TuraService.sync(this)
    }

    /** Android's "share your screen?" prompt for "Show phone on PC". */
    private val mirrorConsent = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { r ->
        val data = r.data
        if (r.resultCode == RESULT_OK && data != null) TuraService.startMirror(this, r.resultCode, data)
    }

    private fun askToMirror() {
        val mpm = getSystemService(android.media.projection.MediaProjectionManager::class.java)
        // The whole screen, not "a single app": that would only show Tunnelrat
        // itself and freeze as soon as you switch apps.
        mirrorConsent.launch(mpm.createScreenCaptureIntent(android.media.projection.MediaProjectionConfig.createConfigForDefaultDisplay()))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (savedInstanceState == null) handle(intent)
        val wanted = buildList {
            if (Build.VERSION.SDK_INT >= 37) add(Manifest.permission.ACCESS_LOCAL_NETWORK)
            if (Build.VERSION.SDK_INT >= 33) add(Manifest.permission.POST_NOTIFICATIONS)
            if (Build.VERSION.SDK_INT >= 31) add(Manifest.permission.BLUETOOTH_CONNECT)
        }.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (wanted.isNotEmpty()) askPermissions.launch(wanted.toTypedArray())
        setContent {
            TunnelratTheme { HomeScreen(vm, onMirror = ::askToMirror) }
        }
    }

    override fun onResume() {
        super.onResume()
        if (Ringer.ringing) Ringer.stop(this) // found it
    }

    /** While the app is open, the phone's volume buttons control the PC's volume. */
    override fun onKeyDown(keyCode: Int, event: KeyEvent?): Boolean {
        // Gamepad mode: volume keys are the shoulder buttons (held = held).
        if (vm.gaming.value && padShoulder(keyCode, down = true)) return true
        // Presenting: volume keys are a clicker (down = next, up = previous).
        // Key repeat is ignored so holding a key doesn't skip slides.
        if (vm.presenting.value && (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN || keyCode == KeyEvent.KEYCODE_VOLUME_UP)) {
            if ((event?.repeatCount ?: 0) == 0) {
                vm.key(if (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN) dev.tunnelrat.ui.SlideKeys.NEXT else dev.tunnelrat.ui.SlideKeys.PREVIOUS)
            }
            return true
        }
        val step = when (keyCode) {
            KeyEvent.KEYCODE_VOLUME_UP -> 5
            KeyEvent.KEYCODE_VOLUME_DOWN -> -5
            else -> return super.onKeyDown(keyCode, event)
        }
        // Only swallow the key if it actually went to the PC; otherwise the phone's own volume changes.
        return vm.volume(step) || super.onKeyDown(keyCode, event)
    }

    override fun onKeyUp(keyCode: Int, event: KeyEvent?): Boolean {
        if (vm.gaming.value && padShoulder(keyCode, down = false)) return true
        return super.onKeyUp(keyCode, event)
    }

    /** Volume down = LB, volume up = RB; false for other keys. */
    private fun padShoulder(keyCode: Int, down: Boolean): Boolean {
        val bit = when (keyCode) {
            KeyEvent.KEYCODE_VOLUME_DOWN -> dev.tunnelrat.gamepad.Pad.LB
            KeyEvent.KEYCODE_VOLUME_UP -> dev.tunnelrat.gamepad.Pad.RB
            else -> return false
        }
        vm.pad.press(bit, down)
        return true
    }

    /** Phone → PC clipboard: Android only allows reading while we're focused. */
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) vm.tura.pushClipboardIfNew()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    /**
     * Handles a pairing link exactly once. Android re-delivers a task's
     * original intent when the app is relaunched from Recents or after the
     * process died, which would resurrect a stale offer, so ignore history
     * launches and clear the link after use.
     */
    private fun handle(intent: Intent?) {
        if (intent?.action == ACTION_MIRROR) {
            setIntent(Intent(this, MainActivity::class.java))
            if (!vm.tura.mirroring.value) askToMirror()
            return
        }
        if (intent?.action != Intent.ACTION_VIEW) return
        if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return
        val uri = intent.data ?: return
        intent.data = null
        setIntent(Intent(this, MainActivity::class.java))
        vm.onPairLink(uri)
    }
}
