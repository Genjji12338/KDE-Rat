package dev.tunnelrat.net

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothSocket
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import java.io.Closeable
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.UUID
import kotlin.concurrent.thread

/**
 * Bluetooth fallback: an RFCOMM connection to the PC's turad, exposed as a
 * one-shot TCP port on 127.0.0.1 so the normal WebSocket-over-TLS client can
 * use it unchanged (TLS still runs end to end with the PC).
 */
class BtBridge private constructor(private val bt: BluetoothSocket) : Closeable {
    private val server = ServerSocket(0, 1, InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1)))
    @Volatile private var local: Socket? = null
    @Volatile private var closed = false

    /** Connect the WebSocket client to 127.0.0.1:[port] (once). */
    val port: Int get() = server.localPort

    init {
        server.soTimeout = 15_000
        thread(name = "tura-bt-accept", isDaemon = true) {
            try {
                val s = server.accept()
                s.tcpNoDelay = true
                local = s
                pump(s.getInputStream(), bt.outputStream, "tura-bt-up")
                pump(bt.inputStream, s.getOutputStream(), "tura-bt-down")
            } catch (e: Exception) {
                android.util.Log.w("TuraBt", "bridge accept failed: $e")
                close()
            } finally {
                runCatching { server.close() }
            }
        }
    }

    private fun pump(from: InputStream, to: OutputStream, name: String) = thread(name = name, isDaemon = true) {
        val buf = ByteArray(16 * 1024)
        try {
            while (true) {
                val n = from.read(buf)
                if (n < 0) break
                to.write(buf, 0, n)
                to.flush()
            }
        } catch (e: Exception) {
            if (!closed) android.util.Log.i("TuraBt", "$name ended: $e")
        } finally {
            close()
        }
    }

    override fun close() {
        if (closed) return
        closed = true
        runCatching { server.close() }
        runCatching { local?.close() }
        runCatching { bt.close() }
    }

    companion object {
        /** Same as turad's `bluetooth::UUID`. */
        val SERVICE: UUID = UUID.fromString("7e1a7a75-9a4e-4b1c-8d2f-74756e6e6c72")

        fun permitted(ctx: Context) =
            Build.VERSION.SDK_INT < 31 || ctx.checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED

        /** Bluetooth is on and we may use it. */
        fun usable(ctx: Context): Boolean {
            if (!permitted(ctx)) return false
            val adapter = ctx.getSystemService(BluetoothManager::class.java)?.adapter ?: return false
            return runCatching { adapter.isEnabled }.getOrDefault(false)
        }

        /**
         * Connects to the PC at [address] ("AA:BB:CC:DD:EE:FF"). Blocks for up to
         * ~10 s; call off the main thread. No Bluetooth pairing needed: the
         * TLS layer on top authenticates both sides.
         */
        @SuppressLint("MissingPermission")
        fun open(ctx: Context, address: String): BtBridge {
            check(permitted(ctx)) { "no Bluetooth permission" }
            val adapter = ctx.getSystemService(BluetoothManager::class.java)?.adapter ?: error("no Bluetooth")
            check(adapter.isEnabled) { "Bluetooth is off" }
            val socket = adapter.getRemoteDevice(address.uppercase()).createInsecureRfcommSocketToServiceRecord(SERVICE)
            runCatching { adapter.cancelDiscovery() } // discovery slows connecting down
            try {
                socket.connect()
            } catch (e: Exception) {
                runCatching { socket.close() }
                throw e
            }
            return BtBridge(socket)
        }
    }
}
