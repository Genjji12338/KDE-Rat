package dev.tunnelrat

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.RingtoneManager
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.VibratorManager
import androidx.core.app.NotificationCompat

/**
 * "Find my phone": a PC asked this phone to ring. Plays the alarm sound on
 * the alarm stream at full volume (so it rings on silent too), vibrates,
 * and shows a notification to stop it. Stops by itself after [MAX_MS].
 */
object Ringer {
    private const val CHANNEL = "ring"
    private const val NOTIF = 7
    private const val MAX_MS = 60_000L
    private const val ACTION_STOP = "dev.tunnelrat.RING_STOP"

    private val main = Handler(Looper.getMainLooper())
    private var player: MediaPlayer? = null
    private var savedVolume: Int? = null
    private val autoStop = Runnable { stop(appContext ?: return@Runnable) }
    private var appContext: Context? = null

    val ringing get() = player != null

    fun start(ctx: Context, pcName: String): Boolean = main.post {
        val app = ctx.applicationContext
        appContext = app
        if (player == null) {
            val audio = app.getSystemService(AudioManager::class.java)
            savedVolume = audio.getStreamVolume(AudioManager.STREAM_ALARM)
            runCatching { audio.setStreamVolume(AudioManager.STREAM_ALARM, audio.getStreamMaxVolume(AudioManager.STREAM_ALARM), 0) }
            val uri = RingtoneManager.getActualDefaultRingtoneUri(app, RingtoneManager.TYPE_ALARM)
                ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE)
            player = runCatching {
                MediaPlayer().apply {
                    setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ALARM).setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build())
                    setDataSource(app, uri)
                    isLooping = true
                    prepare()
                    start()
                }
            }.onFailure { android.util.Log.w("Tunnelrat", "ring: no sound: $it") }.getOrNull() ?: MediaPlayer()
            app.getSystemService(VibratorManager::class.java)?.defaultVibrator
                ?.vibrate(VibrationEffect.createWaveform(longArrayOf(0, 600, 400), 0))
        }
        notify(app, pcName)
        main.removeCallbacks(autoStop)
        main.postDelayed(autoStop, MAX_MS)
    }

    fun stop(ctx: Context): Boolean = main.post {
        val app = ctx.applicationContext
        main.removeCallbacks(autoStop)
        player?.let { runCatching { it.stop() }; it.release() }
        if (player != null) {
            savedVolume?.let { v -> runCatching { app.getSystemService(AudioManager::class.java).setStreamVolume(AudioManager.STREAM_ALARM, v, 0) } }
            app.getSystemService(VibratorManager::class.java)?.defaultVibrator?.cancel()
        }
        player = null
        savedVolume = null
        app.getSystemService(NotificationManager::class.java).cancel(NOTIF)
    }

    private fun notify(app: Context, pcName: String) {
        val nm = app.getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, "Find my phone", NotificationManager.IMPORTANCE_HIGH).apply {
                    description = "When a PC rings this phone"
                    setSound(null, null) // the ringing is ours
                },
            )
        }
        val stop = PendingIntent.getBroadcast(app, 0, Intent(app, StopReceiver::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE)
        val open = PendingIntent.getActivity(app, 0, Intent(app, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        nm.notify(
            NOTIF,
            NotificationCompat.Builder(app, CHANNEL)
                .setSmallIcon(R.drawable.ic_tunnelrat)
                .setContentTitle("Here I am!")
                .setContentText("$pcName is ringing this phone")
                .setCategory(NotificationCompat.CATEGORY_ALARM)
                .setPriority(NotificationCompat.PRIORITY_MAX)
                .setOngoing(true)
                .setContentIntent(open)
                .setDeleteIntent(stop)
                .addAction(0, "Stop", stop)
                .build(),
        )
    }

    class StopReceiver : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            if (intent.action == ACTION_STOP) stop(ctx)
        }
    }
}
