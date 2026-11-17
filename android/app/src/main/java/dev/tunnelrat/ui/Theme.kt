package dev.tunnelrat.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import dev.tunnelrat.R

/**
 * Burrowfolk palette, two themes following the phone's light/dark setting:
 * "Cream · daylight" (ink on cream) and "Ink · basement" (cream on ink).
 * No amber: the one filled accent is ink on cream / cream on ink, red stays
 * rare (Off, offline, the eye in the trackpad corner), cards are outlined in
 * warm grey.
 */
data class TuraPalette(
    val bg: Color,
    /** Cards. */
    val card: Color,
    /** Wells: pressed states, tracks, artwork boxes. */
    val well: Color,
    val line: Color,
    val text: Color,
    val muted: Color,
    /** The one filled accent, and what goes on it. */
    val primary: Color,
    val onPrimary: Color,
    val red: Color,
    val dark: Boolean,
)

val Cream = TuraPalette(
    bg = Color(0xFFF2ECDF), card = Color(0xFFF8F4EB), well = Color(0xFFE6DECD), line = Color(0xFFD6CCBA),
    text = Color(0xFF141414), muted = Color(0xFF5C564C), primary = Color(0xFF141414), onPrimary = Color(0xFFF2ECDF),
    red = Color(0xFFC8321F), dark = false,
)

val Ink = TuraPalette(
    bg = Color(0xFF141414), card = Color(0xFF1C1B19), well = Color(0xFF2A2825), line = Color(0xFF34312C),
    text = Color(0xFFF2ECDF), muted = Color(0xFFA8A196), primary = Color(0xFFF2ECDF), onPrimary = Color(0xFF141414),
    red = Color(0xFFEA5A48), dark = true,
)

/**
 * The app's colours. Plain getters over snapshot state, so they work in any
 * code (draw lambdas too) and everything recomposes when the theme flips.
 * The old names stay so screens didn't need rewriting: "Amber" is now the
 * filled accent (ink/cream), "Bg" is also what goes on it.
 */
object Tura {
    internal var palette by mutableStateOf(Ink)

    val Bg get() = palette.bg
    val Surface get() = palette.card
    val SurfaceHigh get() = palette.well
    val Line get() = palette.line
    val Amber get() = palette.primary
    val AmberDim get() = palette.muted
    val Text get() = palette.text
    val Muted get() = palette.muted
    val Danger get() = palette.red
    /** Online dot: no green in this palette; ink/cream like everything else. */
    val Ok get() = palette.text

    /** Headings and the wordmark. */
    @OptIn(ExperimentalTextApi::class)
    val Display = FontFamily(
        Font(R.font.space_grotesk, FontWeight.Medium, variationSettings = FontVariation.Settings(FontVariation.weight(500))),
        Font(R.font.space_grotesk, FontWeight.Bold, variationSettings = FontVariation.Settings(FontVariation.weight(700))),
    )

    /** Everything else. */
    @OptIn(ExperimentalTextApi::class)
    val Body = FontFamily(
        listOf(400, 500, 600, 700, 800, 900).map { w ->
            Font(R.font.inter, FontWeight(w), variationSettings = FontVariation.Settings(FontVariation.weight(w)))
        },
    )
}

private val BaseTypography = Typography()

private val TuraTypography = Typography(
    displayLarge = BaseTypography.displayLarge.copy(fontFamily = Tura.Display),
    displayMedium = BaseTypography.displayMedium.copy(fontFamily = Tura.Display),
    displaySmall = BaseTypography.displaySmall.copy(fontFamily = Tura.Display),
    headlineLarge = BaseTypography.headlineLarge.copy(fontFamily = Tura.Display),
    headlineMedium = BaseTypography.headlineMedium.copy(fontFamily = Tura.Display),
    headlineSmall = BaseTypography.headlineSmall.copy(fontFamily = Tura.Display),
    titleLarge = BaseTypography.titleLarge.copy(fontFamily = Tura.Display),
    titleMedium = BaseTypography.titleMedium.copy(fontFamily = Tura.Body),
    titleSmall = BaseTypography.titleSmall.copy(fontFamily = Tura.Body),
    bodyLarge = BaseTypography.bodyLarge.copy(fontFamily = Tura.Body),
    bodyMedium = BaseTypography.bodyMedium.copy(fontFamily = Tura.Body),
    bodySmall = BaseTypography.bodySmall.copy(fontFamily = Tura.Body),
    labelLarge = BaseTypography.labelLarge.copy(fontFamily = Tura.Body),
    labelMedium = BaseTypography.labelMedium.copy(fontFamily = Tura.Body),
    labelSmall = BaseTypography.labelSmall.copy(fontFamily = Tura.Body),
)

@Composable
fun TunnelratTheme(content: @Composable () -> Unit) {
    val p = if (isSystemInDarkTheme()) Ink else Cream
    // Root of the tree: written before any child reads it.
    if (Tura.palette != p) Tura.palette = p
    val scheme = if (p.dark) {
        darkColorScheme(
            primary = p.primary, onPrimary = p.onPrimary, background = p.bg, onBackground = p.text,
            surface = p.card, onSurface = p.text, surfaceVariant = p.well, onSurfaceVariant = p.muted,
            outline = p.line, error = p.red,
        )
    } else {
        lightColorScheme(
            primary = p.primary, onPrimary = p.onPrimary, background = p.bg, onBackground = p.text,
            surface = p.card, onSurface = p.text, surfaceVariant = p.well, onSurfaceVariant = p.muted,
            outline = p.line, error = p.red,
        )
    }
    MaterialTheme(colorScheme = scheme, typography = TuraTypography, content = content)
}
