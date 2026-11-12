package dev.tunnelrat.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class DiffEditTest {
    private val s = "​​"

    @Test fun typing() = assertEquals(0 to "a", diffEdit("${s}hell", "${s}hella"))
    @Test fun backspace() = assertEquals(1 to "", diffEdit("${s}hello", "${s}hell"))
    @Test fun backspaceOnEmptyEatsSentinel() = assertEquals(1 to "", diffEdit(s, "​"))
    @Test fun autocorrectRewritesWord() = assertEquals(2 to "he ", diffEdit("${s}teh", "${s}the "))
    @Test fun pastedBlock() = assertEquals(0 to "foo bar\n", diffEdit(s, "${s}foo bar\n"))
}
