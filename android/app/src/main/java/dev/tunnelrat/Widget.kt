package dev.tunnelrat

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.widget.RemoteViews
import android.widget.Toast

/** Home-screen widget with Wake and Off for the paired PC. */
class TuraWidget : AppWidgetProvider() {
    override fun onUpdate(ctx: Context, mgr: AppWidgetManager, ids: IntArray) {
        val name = ctx.tura.pc.value?.name ?: "Not paired"
        val wake = PendingIntent.getBroadcast(ctx, 10, Intent(ctx, WidgetWakeReceiver::class.java), PendingIntent.FLAG_IMMUTABLE)
        val off = PendingIntent.getActivity(
            ctx, 11,
            Intent(ctx, ConfirmOffActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val open = PendingIntent.getActivity(ctx, 12, Intent(ctx, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        for (id in ids) {
            val v = RemoteViews(ctx.packageName, R.layout.widget_tura).apply {
                setTextViewText(R.id.widget_title, name)
                setOnClickPendingIntent(R.id.widget_wake, wake)
                setOnClickPendingIntent(R.id.widget_off, off)
                setOnClickPendingIntent(R.id.widget_title, open)
            }
            mgr.updateAppWidget(id, v)
        }
    }
}

class WidgetWakeReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        val pending = goAsync()
        ctx.tura.wake { msg ->
            Toast.makeText(ctx.applicationContext, msg, Toast.LENGTH_SHORT).show()
            pending.finish()
        }
    }
}

/** Restarts the background connection after a reboot (if the user wants it). */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) TuraService.sync(ctx)
    }
}
