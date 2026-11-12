package dev.tunnelrat

import android.app.Notification
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import dev.tunnelrat.proto.Message

/**
 * Mirrors phone notifications to the PC desktop (only if the user turned it
 * on and granted notification access). Skips ongoing/system/silent clutter,
 * group summaries, our own notifications and "secret" lock-screen ones.
 */
class TuraNotificationListener : NotificationListenerService() {
    companion object {
        /** Reply actions of forwarded notifications, by key (only these can be used from the PC). */
        private val replyActions = java.util.concurrent.ConcurrentHashMap<String, Notification.Action>()

        /** Sends [text] through notification [key]'s own reply field; returns a status line. */
        fun reply(context: android.content.Context, key: String, text: String): String {
            val action = replyActions[key] ?: return "Can't reply: that notification is gone"
            val inputs = action.remoteInputs ?: return "Can't reply to that notification"
            val results = android.os.Bundle().apply { inputs.forEach { putCharSequence(it.resultKey, text) } }
            val intent = android.content.Intent()
            android.app.RemoteInput.addResultsToIntent(inputs, intent, results)
            return try {
                action.actionIntent.send(context, 0, intent)
                "Reply sent"
            } catch (e: android.app.PendingIntent.CanceledException) {
                replyActions.remove(key)
                "Can't reply: the app no longer accepts it"
            }
        }

        private fun replyActionOf(n: Notification): Notification.Action? =
            n.actions?.firstOrNull { a -> a.remoteInputs?.any { it.allowFreeFormInput } == true }
    }

    /** key → last sent content, so progress-style updates don't spam the PC. */
    private val sent = HashMap<String, Int>()

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        val t = applicationContext.tura
        if (!t.shareNotifications.value) return
        val n = sbn.notification
        if (sbn.packageName == packageName || sbn.isOngoing ||
            n.flags and Notification.FLAG_GROUP_SUMMARY != 0 ||
            n.visibility == Notification.VISIBILITY_SECRET ||
            n.category in setOf(Notification.CATEGORY_TRANSPORT, Notification.CATEGORY_PROGRESS, Notification.CATEGORY_SERVICE)
        ) return
        val x = n.extras
        val title = x.getCharSequence(Notification.EXTRA_TITLE)?.toString().orEmpty().take(200)
        val text = (x.getCharSequence(Notification.EXTRA_BIG_TEXT) ?: x.getCharSequence(Notification.EXTRA_TEXT))?.toString().orEmpty().take(1000)
        if (title.isBlank() && text.isBlank()) return
        val replyAction = replyActionOf(n)
        if (replyAction != null) replyActions[sbn.key] = replyAction else replyActions.remove(sbn.key)
        if (replyActions.size > 300) replyActions.clear()
        val hash = (title + "\u0000" + text + (replyAction != null)).hashCode()
        if (sent[sbn.key] == hash) return
        sent[sbn.key] = hash
        if (sent.size > 300) sent.clear()
        val app = runCatching {
            packageManager.getApplicationLabel(packageManager.getApplicationInfo(sbn.packageName, 0)).toString()
        }.getOrDefault(sbn.packageName)
        t.forwardNotification(Message.PhoneNotification(sbn.key, app, title, text, canReply = replyAction != null))
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        replyActions.remove(sbn.key)
        if (sent.remove(sbn.key) != null) {
            applicationContext.tura.forwardNotification(Message.PhoneNotificationRemoved(sbn.key))
        }
    }
}
