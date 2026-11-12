package dev.tunnelrat.cast

import android.content.Context
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Display
import android.view.Surface
import android.view.WindowManager

/**
 * Phone screen → PC: mirrors the screen into a hardware H.264 encoder and
 * hands each access unit to [send] as a binary VIDEO frame. Keyframes carry
 * SPS/PPS so the PC can start decoding at any of them. When the connection
 * backs up ([queued] bytes), frames are skipped and a keyframe is requested.
 */
class ScreenMirror(
    private val context: Context,
    private val projection: MediaProjection,
    private val send: (ByteArray) -> Boolean,
    private val queued: () -> Long,
    private val onStopped: () -> Unit,
) {
    companion object {
        private const val TAG = "TuraMirror"
        private const val MAX_SIDE = 1920
        private const val BACKLOG_BYTES = 1_500_000L

        /**
         * The screen's real size in its current rotation. (An app's own window
         * metrics are stale while it's in the background.)
         */
        fun screenPixels(context: Context): Pair<Int, Int> {
            val d = context.getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
            val (a, b) = d.mode.physicalWidth to d.mode.physicalHeight // natural orientation
            val sideways = d.rotation == Surface.ROTATION_90 || d.rotation == Surface.ROTATION_270
            return if (sideways) b to a else a to b
        }
    }

    /** Current encoded size (changes when the phone rotates). */
    @Volatile var width = 0
        private set
    @Volatile var height = 0
        private set
    private var codec: MediaCodec? = null
    private var input: Surface? = null
    private val thread = HandlerThread("mirror").apply { start() }
    private val handler = Handler(thread.looper)
    private val display: VirtualDisplay
    private val dpi = context.resources.displayMetrics.densityDpi
    private val displays = context.getSystemService(DisplayManager::class.java)
    @Volatile private var running = true
    private var config: ByteArray? = null
    private var seq = 0L
    private var skipping = false

    /** Rotation: re-encode at the new orientation's size. */
    private val rotation = object : DisplayManager.DisplayListener {
        override fun onDisplayChanged(id: Int) {
            if (id == Display.DEFAULT_DISPLAY) resizeIfNeeded()
        }
        override fun onDisplayAdded(id: Int) {}
        override fun onDisplayRemoved(id: Int) {}
    }

    init {
        val (w, h) = screenSize()
        val surface = startEncoder(w, h)
        // Android 14+: register before creating the virtual display.
        projection.registerCallback(
            object : MediaProjection.Callback() {
                override fun onStop() = stop()
            },
            handler,
        )
        display = projection.createVirtualDisplay("Tunnelrat", w, h, dpi, DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface, null, handler)
            ?: throw IllegalStateException("screen capture refused")
        displays.registerDisplayListener(rotation, handler)
    }

    /** The screen in its current orientation, fitted into MAX_SIDE, multiples of 16 (encoder-friendly). */
    private fun screenSize(): Pair<Int, Int> {
        val (w, h) = screenPixels(context)
        val s = minOf(1.0, MAX_SIDE.toDouble() / maxOf(w, h))
        fun r(v: Int) = maxOf(16, ((v * s).toInt() / 16) * 16)
        return r(w) to r(h)
    }

    private fun startEncoder(w: Int, h: Int): Surface {
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, 6_000_000)
            setInteger(MediaFormat.KEY_FRAME_RATE, 30)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)
            // A still screen produces no frames; repeat the last one so the PC
            // keeps getting keyframes (and recovers quickly after a skip).
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 100_000)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            setInteger(MediaFormat.KEY_LATENCY, 1)
            // Say what we send (written into the stream), so the PC converts colours right.
            setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709)
            setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED)
            setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO)
        }
        val c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        c.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        val surface = c.createInputSurface()
        c.setCallback(
            object : MediaCodec.Callback() {
                override fun onInputBufferAvailable(c: MediaCodec, index: Int) {}
                override fun onOutputBufferAvailable(c: MediaCodec, index: Int, info: MediaCodec.BufferInfo) = output(c, index, info)
                override fun onError(c: MediaCodec, e: MediaCodec.CodecException) {
                    Log.w(TAG, "encoder error", e)
                    if (c === codec) stop()
                }
                override fun onOutputFormatChanged(c: MediaCodec, format: MediaFormat) {}
            },
            handler,
        )
        c.start()
        codec = c
        input = surface
        config = null
        width = w
        height = h
        return surface
    }

    private fun resizeIfNeeded() {
        if (!running) return
        val (w, h) = screenSize()
        if (w == width && h == height) return
        Log.i(TAG, "rotated: ${width}x$height → ${w}x$h")
        val oldCodec = codec
        val oldInput = input
        val surface = startEncoder(w, h)
        display.resize(w, h, dpi)
        display.surface = surface
        runCatching { oldCodec?.stop() }
        runCatching { oldCodec?.release() }
        oldInput?.release()
    }

    private fun output(c: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
        // Buffers of an encoder we've just replaced are dropped.
        if (!running || c !== codec) return
        try {
            val buf = c.getOutputBuffer(index)
            if (buf != null && info.size > 0) {
                val data = ByteArray(info.size)
                buf.position(info.offset)
                buf.get(data, 0, info.size)
                if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                    config = data
                } else {
                    val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                    if (key) skipping = false
                    if (!skipping && queued() > BACKLOG_BYTES) {
                        skipping = true
                        c.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
                    }
                    if (!skipping) {
                        val au = if (key) (config ?: ByteArray(0)) + data else data
                        send(frame(++seq, au))
                    }
                }
            }
        } finally {
            if (running) runCatching { c.releaseOutputBuffer(index, false) }
        }
    }

    private fun frame(id: Long, au: ByteArray): ByteArray {
        val out = ByteArray(9 + au.size)
        out[0] = VideoDecoder.KIND
        for (i in 0 until 8) out[1 + i] = (id ushr (56 - 8 * i)).toByte()
        System.arraycopy(au, 0, out, 9, au.size)
        return out
    }

    fun stop() {
        handler.post {
            if (!running) return@post
            running = false
            displays.unregisterDisplayListener(rotation)
            runCatching { display.release() }
            runCatching { codec?.stop() }
            runCatching { codec?.release() }
            input?.release()
            runCatching { projection.stop() }
            thread.quitSafely()
            onStopped()
        }
    }
}
