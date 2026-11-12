package dev.tunnelrat.ui.scan

import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.qrcode.QRCodeWriter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class QrAnalyzerTest {
    private val uri = "tunnelrat://pair?v=1&n=Desk&h=192.168.1.20,100.64.0.20&p=47470" +
        "&fp=5c1e8d0b2a9f47e6c3d8b1a0f9e2d7c4b6a5f8e1d0c3b2a9f8e7d6c5b4a3f2e1&t=0123456789abcdef0123456789abcdef"

    /** Renders [text] as an 8-bit luma frame with extra row padding, like a camera Y plane. */
    private fun frame(text: String, inverted: Boolean, w: Int = 640, h: Int = 480, stride: Int = 704): ByteArray {
        val m = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 360, 360, mapOf(EncodeHintType.MARGIN to 4))
        val ox = (w - m.width) / 2
        val oy = (h - m.height) / 2
        val dark: Byte = if (inverted) 0xE0.toByte() else 0x20
        val light: Byte = if (inverted) 0x20 else 0xE0.toByte()
        val bg: Byte = if (inverted) 0x18 else 0xD0.toByte() // terminal vs paper surround
        return ByteArray(stride * h) { i ->
            val x = i % stride
            val y = i / stride
            val mx = x - ox
            val my = y - oy
            when {
                x >= w -> 0
                mx !in 0 until m.width || my !in 0 until m.height -> bg
                m[mx, my] -> dark
                else -> light
            }
        }
    }

    @Test fun decodesNormalQr() =
        assertEquals(uri, QrAnalyzer {}.decodeLuma(frame(uri, inverted = false), 704, 640, 480))

    @Test fun decodesInvertedTerminalQr() =
        assertEquals(uri, QrAnalyzer {}.decodeLuma(frame(uri, inverted = true), 704, 640, 480))

    @Test fun blankFrameYieldsNothing() =
        assertNull(QrAnalyzer {}.decodeLuma(ByteArray(704 * 480) { 0x80.toByte() }, 704, 640, 480))
}
