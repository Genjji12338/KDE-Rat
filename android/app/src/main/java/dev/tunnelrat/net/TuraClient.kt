package dev.tunnelrat.net

import android.os.Build
import dev.tunnelrat.proto.Message
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit

/**
 * Control channel to turad: WebSocket over mutual TLS.
 *
 * The PC is trusted only if its certificate matches the pinned fingerprint;
 * the PC trusts us by our Keystore client certificate (see [PhoneIdentity]).
 */
class TuraClient {
    companion object {
        /** [State] host while the link runs over Bluetooth. */
        const val BLUETOOTH = "Bluetooth"
    }

    sealed interface State {
        data object Disconnected : State
        data class Connecting(val host: String) : State
        data class Connected(val hostname: String?, val host: String) : State
        data class Failed(val host: String, val reason: String) : State
    }

    sealed interface Event {
        data class Paired(val pcName: String) : Event
        /** The PC rejected us: never paired, token expired, or PC was reset. */
        data class Rejected(val reason: String) : Event
        data class ServerError(val message: String) : Event
        /** The PC's clipboard changed. */
        data class Clipboard(val text: String) : Event
        data class FileSaved(val id: Int, val path: String) : Event
        data class LoginResult(val ok: Boolean, val message: String?) : Event
        /** Bluetooth pairing: the PC is ready ([result] null), or it ended. */
        data class BtPair(val result: Message.BtPairResult?) : Event
    }

    private var socket: WebSocket? = null
    private var http: OkHttpClient? = null
    private var httpPin: String? = null

    private val _state = MutableStateFlow<State>(State.Disconnected)
    val state: StateFlow<State> = _state.asStateFlow()

    private val _pcInfo = MutableStateFlow<Message.PcInfo?>(null)
    val pcInfo: StateFlow<Message.PcInfo?> = _pcInfo.asStateFlow()

    private val _nowPlaying = MutableStateFlow<dev.tunnelrat.proto.NowPlaying?>(null)
    val nowPlaying: StateFlow<dev.tunnelrat.proto.NowPlaying?> = _nowPlaying.asStateFlow()

    private val _events = MutableSharedFlow<Event>(extraBufferCapacity = 32)
    val events: SharedFlow<Event> = _events.asSharedFlow()

    /** Screen cast answers from the PC (CastStarted / CastEnded). */
    private val _cast = MutableSharedFlow<Message>(extraBufferCapacity = 8)
    val cast: SharedFlow<Message> = _cast.asSharedFlow()

    private fun httpFor(pin: String): OkHttpClient {
        http?.takeIf { httpPin == pin }?.let { return it }
        val trust = PinnedTrustManager(pin)
        return OkHttpClient.Builder()
            .socketFactory(NoDelaySocketFactory)
            .sslSocketFactory(pinnedSslContext(trust).socketFactory, trust)
            // Identity is the pinned certificate, not a hostname/IP in it.
            .hostnameVerifier { _, _ -> true }
            .connectTimeout(4, TimeUnit.SECONDS)
            .pingInterval(15, TimeUnit.SECONDS)
            .build()
            .also { http = it; httpPin = pin }
    }

    /** Bluetooth bridge under the current connection, if it goes over Bluetooth. */
    @Volatile private var bridge: BtBridge? = null

    /** Connected (or connecting) over Bluetooth rather than the network. */
    val viaBluetooth: Boolean get() = when (val s = state.value) {
        is State.Connected -> s.host == BLUETOOTH
        is State.Connecting -> s.host == BLUETOOTH
        else -> false
    }

    /**
     * Over Bluetooth to the PC's adapter at [address] (fallback when the
     * network can't reach it). Blocks while Bluetooth connects; call off the main thread.
     */
    fun connectBluetooth(ctx: android.content.Context, pc: PairedPc, address: String, pairToken: String?) {
        socket?.cancel()
        closeBridge()
        _state.value = State.Connecting(BLUETOOTH)
        val b = try {
            BtBridge.open(ctx, address)
        } catch (e: Exception) {
            android.util.Log.w("TuraBt", "Bluetooth connect failed: $e")
            if (state.value == State.Connecting(BLUETOOTH)) _state.value = State.Failed(BLUETOOTH, e.message ?: e.javaClass.simpleName)
            return
        }
        // Superseded (Wi-Fi came back, or disconnect) while Bluetooth was connecting.
        if (state.value != State.Connecting(BLUETOOTH)) {
            android.util.Log.i("TuraBt", "Bluetooth connected but no longer wanted (${state.value})")
            return b.close()
        }
        android.util.Log.i("TuraBt", "Bluetooth connected; starting the session on 127.0.0.1:${b.port}")
        bridge = b
        connect(pc, BLUETOOTH, pairToken, dial = "127.0.0.1", port = b.port)
    }

    private fun closeBridge() {
        bridge?.close()
        bridge = null
    }

    /**
     * @param pairToken one-time token from the QR code, only for the first connection.
     * @param host what to show/report; [dial] and [port] where to actually connect.
     */
    fun connect(pc: PairedPc, host: String, pairToken: String?, dial: String = host, port: Int = pc.port) {
        socket?.cancel()
        if (host != BLUETOOTH) closeBridge()
        _state.value = State.Connecting(host)
        val client = try {
            httpFor(pc.fingerprint)
        } catch (e: Exception) {
            _state.value = State.Failed(host, "TLS setup: ${e.message}")
            return
        }
        var awaitingPair = pairToken != null
        var sawHello = false
        val req = Request.Builder().url("wss://$dial:$port/").build()
        socket = client.newWebSocket(req, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                if (pairToken != null) webSocket.send(Message.Pair(pairToken, Build.MODEL).toJson())
                webSocket.send(Message.Hello(deviceName = Build.MODEL).toJson())
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                when (val m = runCatching { Message.fromJson(text) }.getOrNull()) {
                    is Message.PairOk -> {
                        awaitingPair = false
                        _events.tryEmit(Event.Paired(m.pcName))
                    }
                    is Message.Hello -> {
                        sawHello = true
                        _state.value = State.Connected(null, host)
                    }
                    is Message.NowPlayingMsg -> _nowPlaying.value = m.np
                    is Message.FsRequest -> onFsRequest?.invoke(m.id, m.op)
                    is Message.Clipboard -> _events.tryEmit(Event.Clipboard(m.text))
                    is Message.FileSaved -> _events.tryEmit(Event.FileSaved(m.id, m.path))
                    is Message.CastStarted, is Message.CastEnded -> _cast.tryEmit(m)
                    is Message.LoginResult -> _events.tryEmit(Event.LoginResult(m.ok, m.message))
                    is Message.BtPairReady -> _events.tryEmit(Event.BtPair(null))
                    is Message.BtPairResult -> _events.tryEmit(Event.BtPair(m))
                    is Message.MirrorStop, is Message.MirrorRequest, is Message.Ring, is Message.PhoneReply, is Message.PhoneGesture, is Message.PhoneTouch, is Message.PhoneNav, is Message.PhoneText -> onControl?.invoke(m)
                    is Message.PcInfo -> {
                        _pcInfo.value = m
                        _state.value = State.Connected(m.hostname, host)
                    }
                    // An error before the PC's Hello means the pairing gate said no.
                    is Message.Error -> _events.tryEmit(if (!sawHello || awaitingPair) Event.Rejected(m.message) else Event.ServerError(m.message))
                    is Message.Ping -> webSocket.send(Message.Pong.toJson())
                    else -> Unit
                }
            }

            override fun onMessage(webSocket: WebSocket, bytes: okio.ByteString) {
                onBinary?.invoke(bytes.toByteArray())
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                if (socket === webSocket) {
                    closeBridge()
                    _state.value = State.Disconnected
                }
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                if (host == BLUETOOTH) android.util.Log.w("TuraBt", "link over Bluetooth failed (current=${socket === webSocket})", t)
                if (socket === webSocket) {
                    closeBridge()
                    _state.value = State.Failed(host, t.message ?: t.javaClass.simpleName)
                }
            }
        })
    }

    fun disconnect() {
        socket?.close(1000, null)
        socket = null
        closeBridge()
        _state.value = State.Disconnected
    }

    /**
     * File requests from the PC. Called on OkHttp's reader thread; a direct
     * callback (not [events]) so bursts of requests are never dropped.
     */
    @Volatile var onFsRequest: ((Long, org.json.JSONObject) -> Unit)? = null

    /** Mirroring: PC input and "viewer closed". Called on OkHttp's reader thread. */
    @Volatile var onControl: ((Message) -> Unit)? = null

    /** Binary frames from the PC (drive writes). Same thread rules as [onFsRequest]. */
    @Volatile var onBinary: ((ByteArray) -> Unit)? = null

    /** Bytes queued for sending but not yet written (for upload back-pressure). */
    fun queueSize(): Long = socket?.queueSize() ?: 0L

    fun sendBytes(data: ByteArray): Boolean =
        state.value is State.Connected && (socket?.send(okio.ByteString.of(*data)) ?: false)

    fun send(m: Message): Boolean =
        state.value is State.Connected && (socket?.send(m.toJson()) ?: false)
}

/**
 * Disables Nagle's algorithm: replies (e.g. drive reads) are written in one
 * burst and shouldn't wait for the PC's ACK of the previous segment.
 */
private object NoDelaySocketFactory : javax.net.SocketFactory() {
    private val base = javax.net.SocketFactory.getDefault()
    private fun java.net.Socket.tuned() = apply { tcpNoDelay = true }
    override fun createSocket() = base.createSocket().tuned()
    override fun createSocket(host: String?, port: Int) = base.createSocket(host, port).tuned()
    override fun createSocket(host: String?, port: Int, local: java.net.InetAddress?, localPort: Int) =
        base.createSocket(host, port, local, localPort).tuned()
    override fun createSocket(host: java.net.InetAddress?, port: Int) = base.createSocket(host, port).tuned()
    override fun createSocket(addr: java.net.InetAddress?, port: Int, local: java.net.InetAddress?, localPort: Int) =
        base.createSocket(addr, port, local, localPort).tuned()
}
