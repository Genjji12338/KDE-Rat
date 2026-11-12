package dev.tunnelrat.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.tunnelrat.proto.Modifier as Mod

/** evdev key codes for the special-key bar. */
private object K {
    const val ESC = 1; const val TAB = 15; const val BACKSPACE = 14; const val ENTER = 28
    const val HOME = 102; const val UP = 103; const val PGUP = 104; const val LEFT = 105
    const val RIGHT = 106; const val END = 107; const val DOWN = 108; const val PGDN = 109; const val DEL = 111
}

/**
 * Two zero-width spaces the hidden field always starts with, so a Backspace
 * on an "empty" field still produces a change we can see.
 */
private const val SENTINEL = "​​"
private fun fresh() = TextFieldValue(SENTINEL, TextRange(SENTINEL.length))

/**
 * Turns an edit of the hidden field into what the PC should type:
 * characters removed from the end → Backspaces, new characters → text.
 * Works with IME composition and autocorrect, which rewrite whole words.
 */
internal fun diffEdit(old: String, new: String): Pair<Int, String> {
    var p = 0
    while (p < old.length && p < new.length && old[p] == new[p]) p++
    return (old.length - p) to new.substring(p).replace("​", "")
}

/**
 * Phone-keyboard typing for the PC: a hidden text field the IME types into,
 * plus a bar of keys phones don't have. Ctrl/Alt/Super are one-shot toggles
 * that apply to the next key or character (Ctrl → c = copy).
 */
@Composable
fun KeyboardBar(
    onText: (String, Set<Mod>) -> Unit,
    onKey: (Int, Set<Mod>) -> Unit,
    modifier: Modifier = Modifier,
) {
    val haptics = LocalHapticFeedback.current
    val ime = LocalSoftwareKeyboardController.current
    val focus = remember { FocusRequester() }
    var field by remember { mutableStateOf(fresh()) }
    var mods by remember { mutableStateOf(emptySet<Mod>()) }

    LaunchedEffect(Unit) {
        focus.requestFocus()
        ime?.show()
    }

    fun consumeMods(): Set<Mod> = mods.also { mods = emptySet() }

    Row(
        modifier.height(44.dp).horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        // The IME's target. Invisible but focusable; tapping the bar re-opens the keyboard.
        BasicTextField(
            value = field,
            onValueChange = { new ->
                val (backspaces, typed) = diffEdit(field.text, new.text)
                if (backspaces > 0) onText("\b".repeat(backspaces), emptySet())
                if (typed.isNotEmpty()) onText(typed, consumeMods())
                // Keep the buffer tiny and the sentinel intact; keep composition while it lasts.
                field = if (!new.text.startsWith(SENTINEL) || new.text.length > 48 || '\n' in new.text) fresh() else new
            },
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Text,
            ),
            modifier = Modifier.size(1.dp).alpha(0f).focusRequester(focus),
        )

        for ((m, label) in listOf(Mod.Ctrl to "Ctrl", Mod.Alt to "Alt", Mod.Super to "Super")) {
            BarKey(label, active = m in mods) { mods = if (m in mods) mods - m else mods + m }
        }
        BarKey("Esc") { onKey(K.ESC, consumeMods()) }
        BarKey("Tab") { onKey(K.TAB, consumeMods()) }
        BarKey("←") { onKey(K.LEFT, consumeMods()) }
        BarKey("↑") { onKey(K.UP, consumeMods()) }
        BarKey("↓") { onKey(K.DOWN, consumeMods()) }
        BarKey("→") { onKey(K.RIGHT, consumeMods()) }
        BarKey("Home") { onKey(K.HOME, consumeMods()) }
        BarKey("End") { onKey(K.END, consumeMods()) }
        BarKey("PgUp") { onKey(K.PGUP, consumeMods()) }
        BarKey("PgDn") { onKey(K.PGDN, consumeMods()) }
        BarKey("Del") { onKey(K.DEL, consumeMods()) }
        BarKey("⌫") { onKey(K.BACKSPACE, consumeMods()) }
        BarKey("⏎") { onKey(K.ENTER, consumeMods()) }
        BarKey("⌨") {
            haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
            focus.requestFocus()
            ime?.show()
        }
    }
}

@Composable
private fun BarKey(label: String, active: Boolean = false, onClick: () -> Unit) {
    val haptics = LocalHapticFeedback.current
    val shape = RoundedCornerShape(12.dp)
    Box(
        Modifier.height(40.dp).widthIn(min = 44.dp).clip(shape)
            .background(if (active) Tura.Amber else Tura.Surface)
            .border(1.dp, if (active) Color.Transparent else Tura.Line, shape)
            .clickable { haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove); onClick() }
            .padding(horizontal = 12.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = if (active) Tura.Bg else Tura.Text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
    }
}
