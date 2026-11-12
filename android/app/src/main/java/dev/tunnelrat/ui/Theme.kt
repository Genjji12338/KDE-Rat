package dev.tunnelrat.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

/** Tunnel palette: near-black walls, warm lamp-amber light at the end. */
object Tura {
    val Bg = Color(0xFF0B0B0A)
    val Surface = Color(0xFF171614)
    val SurfaceHigh = Color(0xFF22201C)
    val Line = Color(0xFF2E2B26)
    val Amber = Color(0xFFFFB224)
    val AmberDim = Color(0xFF6B4A0E)
    val Text = Color(0xFFF4EFE6)
    val Muted = Color(0xFF8E877C)
    val Danger = Color(0xFFFF5A4E)
    val Ok = Color(0xFF7BD88F)
}

@Composable
fun TunnelratTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Tura.Amber,
            onPrimary = Tura.Bg,
            background = Tura.Bg,
            onBackground = Tura.Text,
            surface = Tura.Surface,
            onSurface = Tura.Text,
            surfaceVariant = Tura.SurfaceHigh,
            onSurfaceVariant = Tura.Muted,
            outline = Tura.Line,
            error = Tura.Danger,
        ),
        content = content,
    )
}
