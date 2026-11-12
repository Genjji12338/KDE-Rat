package dev.tunnelrat

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import dev.tunnelrat.net.TuraClient
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.launch

/**
 * Keeps the process (and so the PC connection) alive in the background so
 * clipboard, notifications and battery work while the app isn't open.
 * Android requires a visible notification for that; it doubles as a status
 * line with "Send clipboard" and "Stop" actions.
 */
class TuraService : Service() {
    private var watch: Job? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_MIRROR_DONE) {
            // Back to a plain connection service (or none).
            if (tura.pc.value != null && tura.keepConnected.value) {
                startForeground(NOTIF_ID, build(tura.client.state.value), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            } else {
                stopSelf()
            }
            return START_STICKY
        }
        if (intent?.action == ACTION_MIRROR) {
            ensureChannel(this)
            // Must be a mediaProjection service before the projection is created.
            startForeground(
                NOTIF_ID,
                build(tura.client.state.value),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION,
            )
            val data = intent.getParcelableExtra(EXTRA_DATA, Intent::class.java)
            if (data != null) tura.startMirror(intent.getIntExtra(EXTRA_CODE, 0), data)
            return START_NOT_STICKY
        }
        if (intent?.action == ACTION_STOP) {
            tura.set(tura.keepConnected, "keep_connected", false)
            stopSelf()
            return START_NOT_STICKY
        }
        ensureChannel(this)
        startForeground(NOTIF_ID, build(tura.client.state.value), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        if (watch == null) {
            watch = tura.scope.launch {
                // Any PC's state, the PC list or the network changing → refresh the text.
                @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
                kotlinx.coroutines.flow.combine(tura.links, tura.net, tura.pc) { links, _, _ -> links }
                    .flatMapLatest { links -> kotlinx.coroutines.flow.combine(links.map { it.client.state } + kotlinx.coroutines.flow.flowOf(null)) { it } }
                    .collect { getSystemService(NotificationManager::class.java).notify(NOTIF_ID, build(tura.client.state.value)) }
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        watch?.cancel()
        watch = null
        super.onDestroy()
    }

    private fun build(st: TuraClient.State): Notification {
        val name = tura.pc.value?.name ?: "PC"
        val links = tura.links.value
        val text = when {
            links.size > 1 -> {
                val on = links.filter { it.isConnected }.map { it.pc.value.name }
                if (on.isEmpty()) "No PC reachable — retrying" else "Connected to ${on.joinToString(", ")}"
            }
            st is TuraClient.State.Connected -> "Connected to ${st.hostname ?: name}"
            tura.net.value == Tura.Net.MobileOnly -> "Not on Wi-Fi — $name unreachable"
            tura.net.value == Tura.Net.None -> "No network"
            st is TuraClient.State.Connecting -> "Looking for $name…"
            else -> "$name is offline — retrying"
        }
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val sendClip = PendingIntent.getActivity(
            this, 1,
            Intent(this, ClipboardSendActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(this, 2, Intent(this, TuraService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_tunnelrat)
            .setContentTitle("Tunnelrat")
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_SERVICE)
            .addAction(Notification.Action.Builder(null, "Send clipboard", sendClip).build())
            .addAction(Notification.Action.Builder(null, "Stop", stop).build())
            .build()
    }

    companion object {
        private const val CHANNEL = "connection"
        private const val NOTIF_ID = 1
        private const val ACTION_STOP = "dev.tunnelrat.STOP"
        private const val ACTION_MIRROR = "dev.tunnelrat.MIRROR"
        private const val ACTION_MIRROR_DONE = "dev.tunnelrat.MIRROR_DONE"
        private const val EXTRA_CODE = "code"
        private const val EXTRA_DATA = "data"

        /** After the user allowed screen capture: start mirroring from the service. */
        fun startMirror(ctx: Context, code: Int, data: Intent) {
            ctx.startForegroundService(Intent(ctx, TuraService::class.java).setAction(ACTION_MIRROR).putExtra(EXTRA_CODE, code).putExtra(EXTRA_DATA, data))
        }

        fun mirrorEnded(ctx: Context) {
            runCatching { ctx.startService(Intent(ctx, TuraService::class.java).setAction(ACTION_MIRROR_DONE)) }
        }

        fun ensureChannel(ctx: Context) {
            val nm = ctx.getSystemService(NotificationManager::class.java)
            if (nm.getNotificationChannel(CHANNEL) == null) {
                nm.createNotificationChannel(
                    NotificationChannel(CHANNEL, "Connection", NotificationManager.IMPORTANCE_LOW).apply {
                        description = "Keeps Tunnelrat connected to your PC in the background"
                        setShowBadge(false)
                    },
                )
            }
        }

        /** Starts or stops the service to match "paired && keep connected". */
        fun sync(ctx: Context) {
            val t = ctx.tura
            val want = t.pc.value != null && t.keepConnected.value
            val intent = Intent(ctx, TuraService::class.java)
            try {
                if (want) ctx.startForegroundService(intent) else ctx.stopService(intent)
            } catch (e: Exception) {
                // Android may refuse background starts (e.g. right after boot); the
                // app still works in the foreground and retries on next launch.
                android.util.Log.w("Tunnelrat", "service start refused: $e")
            }
        }
    }
}
