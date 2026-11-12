package dev.tunnelrat.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.Base64
import android.util.LruCache
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.MusicNote
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.SkipNext
import androidx.compose.material.icons.rounded.SkipPrevious
import androidx.compose.material.icons.rounded.VolumeDown
import androidx.compose.material.icons.rounded.VolumeOff
import androidx.compose.material.icons.rounded.VolumeUp
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.tunnelrat.proto.MediaAction
import dev.tunnelrat.proto.NowPlaying
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.TimeUnit

private val CardShape = RoundedCornerShape(24.dp)

/** Now playing on the PC + system volume. */
@Composable
fun MediaCard(
    np: NowPlaying?,
    onMedia: (MediaAction) -> Unit,
    onVolume: (Int) -> Unit,
    onMute: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val hasPlayer = np?.player != null
    Column(
        modifier.background(Tura.Surface, CardShape).border(1.dp, Tura.Line, CardShape).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            AlbumArt(np?.art, size = 56.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    if (hasPlayer) np!!.title.ifBlank { np.player!! } else "Nothing playing",
                    color = if (hasPlayer) Tura.Text else Tura.Muted,
                    fontWeight = FontWeight.Bold,
                    fontSize = 15.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    when {
                        !hasPlayer -> "Start music on the PC"
                        np!!.artist.isNotBlank() -> np.artist
                        else -> np.player!!
                    },
                    color = Tura.Muted,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            CircleIcon(Icons.Rounded.SkipPrevious, "Previous", enabled = hasPlayer && np!!.canPrevious) { onMedia(MediaAction.Previous) }
            CircleIcon(
                if (np?.playing == true) Icons.Rounded.Pause else Icons.Rounded.PlayArrow,
                if (np?.playing == true) "Pause" else "Play",
                enabled = hasPlayer,
                filled = true,
                size = 48.dp,
            ) { onMedia(MediaAction.PlayPause) }
            CircleIcon(Icons.Rounded.SkipNext, "Next", enabled = hasPlayer && np!!.canNext) { onMedia(MediaAction.Next) }
        }

        // Volume: mute · − · bar · +
        Row(Modifier.fillMaxWidth().height(36.dp), verticalAlignment = Alignment.CenterVertically) {
            val muted = np?.muted == true
            CircleIcon(if (muted) Icons.Rounded.VolumeOff else Icons.Rounded.VolumeUp, if (muted) "Unmute" else "Mute", tint = if (muted) Tura.Danger else Tura.Text, size = 36.dp, onClick = onMute)
            CircleIcon(Icons.Rounded.VolumeDown, "Volume down", size = 36.dp) { onVolume(-5) }
            VolumeBar((np?.volume ?: 0) / 100f, muted, Modifier.weight(1f).padding(horizontal = 6.dp))
            Text("${np?.volume ?: 0}%", color = Tura.Muted, fontSize = 12.sp, modifier = Modifier.width(40.dp))
            CircleIcon(Icons.Rounded.VolumeUp, "Volume up", size = 36.dp) { onVolume(5) }
        }
    }
}

@Composable
private fun VolumeBar(level: Float, muted: Boolean, modifier: Modifier) {
    val shown by animateFloatAsState(level.coerceIn(0f, 1f), label = "vol")
    Box(modifier.height(6.dp).clip(CircleShape).background(Tura.SurfaceHigh)) {
        Box(Modifier.fillMaxHeight().fillMaxWidth(shown).clip(CircleShape).background(if (muted) Tura.Muted else Tura.Amber))
    }
}

@Composable
private fun CircleIcon(
    icon: ImageVector,
    description: String,
    enabled: Boolean = true,
    filled: Boolean = false,
    size: Dp = 40.dp,
    tint: Color = Tura.Text,
    onClick: () -> Unit,
) {
    val haptics = LocalHapticFeedback.current
    Box(
        Modifier.size(size).clip(CircleShape)
            .background(if (filled && enabled) Tura.Amber else Color.Transparent)
            .clickable(enabled = enabled) { haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove); onClick() },
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            icon,
            contentDescription = description,
            tint = when {
                !enabled -> Tura.Line
                filled -> Tura.Bg
                else -> tint
            },
            modifier = Modifier.size(size * 0.58f),
        )
    }
}

// ── Cover art ──

private val artHttp by lazy {
    OkHttpClient.Builder().callTimeout(8, TimeUnit.SECONDS).build()
}
private val artCache = LruCache<String, ImageBitmap>(8)

private suspend fun loadArt(src: String): ImageBitmap? = withContext(Dispatchers.IO) {
    artCache.get(src)?.let { return@withContext it }
    val bytes: ByteArray? = runCatching {
        when {
            src.startsWith("data:") -> Base64.decode(src.substringAfter("base64,"), Base64.DEFAULT)
            src.startsWith("https://") -> artHttp.newCall(Request.Builder().url(src).build()).execute().use { r ->
                if (r.isSuccessful) r.body.bytes() else null
            }
            else -> null
        }
    }.getOrNull()
    val bmp: Bitmap = bytes?.let { BitmapFactory.decodeByteArray(it, 0, it.size) } ?: return@withContext null
    bmp.asImageBitmap().also { artCache.put(src, it) }
}

@Composable
private fun AlbumArt(src: String?, size: Dp) {
    val art by produceState<ImageBitmap?>(null, src) { value = src?.let { loadArt(it) } }
    Box(
        Modifier.size(size).clip(RoundedCornerShape(14.dp)).background(Tura.SurfaceHigh),
        contentAlignment = Alignment.Center,
    ) {
        Crossfade(art, label = "art") { bmp ->
            if (bmp != null) {
                Image(bmp, contentDescription = "Cover art", contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
            } else {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Icon(Icons.Rounded.MusicNote, contentDescription = null, tint = Tura.Muted)
                }
            }
        }
    }
}
