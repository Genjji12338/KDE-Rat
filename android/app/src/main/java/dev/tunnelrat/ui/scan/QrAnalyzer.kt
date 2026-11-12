package dev.tunnelrat.ui.scan

import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.LuminanceSource
import com.google.zxing.MultiFormatReader
import com.google.zxing.NotFoundException
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Decodes QR codes from CameraX frames using only the Y (luminance) plane.
 *
 * Each frame is tried as-is and inverted: `tura pair` draws light modules on
 * a dark terminal background, which plain ZXing would not find.
 */
class QrAnalyzer(private val onText: (String) -> Unit) : ImageAnalysis.Analyzer {
    private val reader = MultiFormatReader().apply {
        setHints(
            mapOf(
                DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
                DecodeHintType.TRY_HARDER to true,
            ),
        )
    }
    private val paused = AtomicBoolean(false)
    private var buf = ByteArray(0)

    /** Stop decoding (e.g. while a result is being shown). */
    fun pause(p: Boolean) = paused.set(p)

    override fun analyze(image: ImageProxy) {
        image.use {
            if (paused.get()) return
            val plane = it.planes[0]
            val rowStride = plane.rowStride
            val size = plane.buffer.remaining()
            if (buf.size < size) buf = ByteArray(size)
            plane.buffer.get(buf, 0, size)

            val text = decodeLuma(buf, rowStride, it.width, it.height) ?: return
            onText(text)
        }
    }

    /** Decodes a Y plane; tries normal, then inverted (light-on-dark) codes. */
    fun decodeLuma(data: ByteArray, rowStride: Int, width: Int, height: Int): String? {
        val source = PlanarYUVLuminanceSource(data, rowStride, height, 0, 0, width, height, false)
        return decode(source) ?: decode(source.invert())
    }

    private fun decode(src: LuminanceSource): String? = try {
        reader.decodeWithState(BinaryBitmap(HybridBinarizer(src))).text
    } catch (_: NotFoundException) {
        null
    } catch (_: Exception) {
        null // checksum/format errors on partial frames are expected
    } finally {
        reader.reset()
    }
}
