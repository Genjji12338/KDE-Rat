package dev.tunnelrat.cast

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface

/**
 * Hardware H.264 decoder for the PC's screen cast, rendering straight onto
 * [surface]. Frames come from the network thread via [feed]; everything
 * touching the codec runs on one handler thread.
 *
 * Until a keyframe arrives (and after falling behind) non-key frames are
 * skipped: they'd only decode into garbage.
 */
class VideoDecoder(surface: Surface, width: Int, height: Int) {
    companion object {
        /** Binary frame kind (`tura_proto::bin::VIDEO`). */
        const val KIND: Byte = 4
        private const val HEADER = 9
        /** Frames waiting for a decoder buffer before we skip to the next keyframe. */
        private const val MAX_PENDING = 6
        private const val TAG = "TuraCast"

        fun isKeyframe(d: ByteArray, from: Int): Boolean {
            var i = from
            while (i + 3 < d.size) {
                if (d[i].toInt() == 0 && d[i + 1].toInt() == 0 && d[i + 2].toInt() == 1) {
                    val type = d[i + 3].toInt() and 0x1f
                    if (type == 5 || type == 7) return true
                    i += 3
                } else {
                    i++
                }
            }
            return false
        }
    }

    private val thread = HandlerThread("cast-decoder").apply { start() }
    private val handler = Handler(thread.looper)
    private val codec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
    private val pending = ArrayDeque<ByteArray>()
    private val freeInputs = ArrayDeque<Int>()
    private var waitingForKey = true
    private var released = false

    init {
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height).apply {
            setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0) // real time
        }
        codec.setCallback(
            object : MediaCodec.Callback() {
                override fun onInputBufferAvailable(c: MediaCodec, index: Int) {
                    freeInputs.addLast(index)
                    pump()
                }

                override fun onOutputBufferAvailable(c: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                    if (!released) c.releaseOutputBuffer(index, true)
                }

                override fun onError(c: MediaCodec, e: MediaCodec.CodecException) {
                    Log.w(TAG, "decoder error", e)
                    waitingForKey = true
                }

                override fun onOutputFormatChanged(c: MediaCodec, format: MediaFormat) {}
            },
            handler,
        )
        codec.configure(format, surface, null, 0)
        codec.start()
    }

    /** One binary frame from the PC (9-byte header + one access unit). */
    fun feed(frame: ByteArray) {
        if (frame.size <= HEADER) return
        handler.post {
            if (released) return@post
            val key = isKeyframe(frame, HEADER)
            if (waitingForKey && !key) return@post
            if (pending.size >= MAX_PENDING) {
                // Too far behind: drop the backlog and resync on a keyframe.
                pending.clear()
                if (!key) {
                    waitingForKey = true
                    return@post
                }
            }
            waitingForKey = false
            pending.addLast(frame)
            pump()
        }
    }

    private fun pump() {
        while (!released && pending.isNotEmpty() && freeInputs.isNotEmpty()) {
            val frame = pending.removeFirst()
            val index = freeInputs.removeFirst()
            try {
                val buf = codec.getInputBuffer(index) ?: continue
                val len = minOf(frame.size - HEADER, buf.capacity())
                buf.clear()
                buf.put(frame, HEADER, len)
                codec.queueInputBuffer(index, 0, len, System.nanoTime() / 1000, 0)
            } catch (e: IllegalStateException) {
                Log.w(TAG, "queue failed", e)
                waitingForKey = true
            }
        }
    }

    fun release() {
        handler.post {
            released = true
            runCatching { codec.stop() }
            runCatching { codec.release() }
            thread.quitSafely()
        }
    }
}
