package dev.tunnelrat

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.Toast
import androidx.core.content.IntentCompat

/** "Send to PC" in the Android share sheet: links open on the PC, text goes to its clipboard, files to Downloads. */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val t = tura
        if (t.pc.value == null) {
            Toast.makeText(this, "Pair Tunnelrat with your PC first", Toast.LENGTH_LONG).show()
            return finish()
        }
        val uris: List<Uri> = when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE -> IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> emptyList()
        }
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
        // The share sheet closes right away; report the outcome as a toast.
        val report: (String) -> Unit = {
            android.util.Log.i("Tunnelrat", "share: $it")
            Toast.makeText(applicationContext, it, Toast.LENGTH_SHORT).show()
        }
        when {
            uris.isNotEmpty() -> {
                // Open now, while this activity holds the read grant.
                val opened = uris.take(20).mapNotNull { t.openShared(it) }
                if (opened.isEmpty()) report("Couldn't read the shared file")
                opened.forEach { t.shareFile(it, report) }
            }
            !text.isNullOrBlank() -> t.shareText(text, report)
            else -> Toast.makeText(this, "Nothing to send", Toast.LENGTH_SHORT).show()
        }
        finish()
    }
}
