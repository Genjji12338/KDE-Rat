package dev.tunnelrat

import android.app.Application
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.Uri
import android.os.BatteryManager
import android.os.Build
import android.os.PersistableBundle
import android.provider.OpenableColumns
import android.util.Base64
import dev.tunnelrat.net.PairOffer
import dev.tunnelrat.net.PairedPc
import dev.tunnelrat.net.TuraClient
import dev.tunnelrat.net.WakeOnLan
import dev.tunnelrat.net.parsePairUri
import dev.tunnelrat.proto.FILE_CHUNK
import dev.tunnelrat.proto.Message
import dev.tunnelrat.proto.PowerAction
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.atomic.AtomicInteger

/**
 * The app's connections to its paired PCs, alive as long as the process is
 * (kept alive by [TuraService]). Every paired PC stays connected, so anything
 * a PC starts (mirroring, the drive, sending files) works from any of them.
 * What the user starts on the phone goes to the *selected* PC ([pc], [client]).
 * Activities, the share target, the tile and the widget all go through this.
 */
private const val REQUESTS = "requests"
private const val MIRROR_REQUEST_NOTIF = 2

class Tura(private val app: Application) {
    private val prefs = app.getSharedPreferences("tunnelrat", Context.MODE_PRIVATE)
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /** One per paired PC. */
    inner class PcLink(initial: PairedPc, var pendingToken: String?) {
        val client = TuraClient()
        val pc = MutableStateFlow(initial)
        val fingerprint get() = pc.value.fingerprint
        var hostIdx = 0
        /** Remembered from the last PcInfo so we can wake the PC while it's off. */
        var wakeMac: String? = prefs.getString("wake_mac_${initial.fingerprint}", null)
        private val jobs = mutableListOf<kotlinx.coroutines.Job>()

        /** The PC's Bluetooth address, from its last PcInfo: the fallback route. */
        var btAddress: String? = prefs.getString("bt_${initial.fingerprint}", null)
        /** Network attempts since the last success or Bluetooth try. */
        var netTries = 0
        private var lastBtTry = 0L
        private var lastProbe = 0L

        val isConnected get() = client.state.value is TuraClient.State.Connected

        fun connect() {
            val p = pc.value
            if (p.hosts.isEmpty()) return
            client.connect(p, p.hosts[Math.floorMod(hostIdx, p.hosts.size)], pendingToken)
        }

        /** Bluetooth is possible for this PC right now. */
        fun canBluetooth() = btAddress != null && pendingToken == null && dev.tunnelrat.net.BtBridge.usable(app)

        /** Tries Bluetooth (at most every 20 s: each try pages the PC's radio). */
        fun connectBluetooth() {
            val addr = btAddress ?: return
            val now = android.os.SystemClock.elapsedRealtime()
            if (now - lastBtTry < 20_000) return
            lastBtTry = now
            netTries = 0
            val p = pc.value
            scope.launch(Dispatchers.IO) { client.connectBluetooth(app, p, addr, null) }
        }

        /**
         * On Bluetooth with a network back: if the PC answers there, switch
         * (checked every 30 s with a plain TCP connect, which is cheap).
         */
        fun maybeLeaveBluetooth(force: Boolean = false) {
            if (!client.viaBluetooth || client.state.value !is TuraClient.State.Connected || !net.value.canReachPc) return
            val now = android.os.SystemClock.elapsedRealtime()
            if (!force && now - lastProbe < 30_000) return
            lastProbe = now
            val p = pc.value
            scope.launch(Dispatchers.IO) {
                val reachable = p.hosts.withIndex().firstOrNull { (_, h) ->
                    runCatching { java.net.Socket().use { it.connect(java.net.InetSocketAddress(h, p.port), 1_500) } }.isSuccess
                } ?: return@launch
                kotlinx.coroutines.withContext(Dispatchers.Main) {
                    if (client.viaBluetooth) {
                        hostIdx = reachable.index
                        connect()
                    }
                }
            }
        }

        fun start() {
            jobs += scope.launch {
                client.pcInfo.collect { info ->
                    info?.bluetooth?.let { bt ->
                        if (bt != btAddress) {
                            btAddress = bt
                            prefs.edit().putString("bt_$fingerprint", bt).apply()
                        }
                    }
                    val mac = info?.wolMacs?.firstOrNull() ?: return@collect
                    wakeMac = mac
                    prefs.edit().putString("wake_mac_$fingerprint", mac).apply()
                }
            }
            jobs += scope.launch { client.events.collect { onEvent(this@PcLink, it) } }
            // On every (re)connect, report battery and offer the drive right away.
            jobs += scope.launch {
                client.state.collect { st ->
                    updateDiscovery()
                    if (st is TuraClient.State.Connected) {
                        battery?.let { client.send(it) }
                        if (mirrorLink === this@PcLink) mirror?.let { client.send(Message.MirrorStart(it.width, it.height)) }
                        offerDrive(this@PcLink)
                    }
                }
            }
            // Keep digging. Polls instead of collecting: StateFlow drops a repeated,
            // identical Failed(...), which would stall a collector-driven retry.
            jobs += scope.launch {
                var backoff = 1_000L
                while (true) {
                    delay(backoff)
                    when (client.state.value) {
                        is TuraClient.State.Connected -> {
                            backoff = 1_000L
                            netTries = 0
                            maybeLeaveBluetooth()
                        }
                        is TuraClient.State.Connecting -> backoff = 1_000L
                        is TuraClient.State.Failed, TuraClient.State.Disconnected -> {
                            // The network first (each of the PC's addresses: LAN, then
                            // Tailscale); when that has no route home or keeps failing,
                            // Bluetooth if the PC has it. Then the network again.
                            val p = pc.value
                            if (net.value.canReachPc && (netTries <= p.hosts.size || !canBluetooth())) {
                                netTries++
                                hostIdx++
                                connect()
                            } else if (canBluetooth()) {
                                connectBluetooth()
                            }
                            backoff = (backoff * 2).coerceAtMost(10_000L)
                        }
                    }
                }
            }
            client.onFsRequest = { id, op ->
                val fs = currentFs()
                scope.launch(fsDispatcher) {
                    val t0 = System.nanoTime()
                    try {
                        handleFsRequest(this@PcLink, fs, id, op)
                    } catch (e: Throwable) {
                        android.util.Log.e("TuraFs", "req $id crashed", e)
                        reply(this@PcLink, id, dev.tunnelrat.drive.fsError("io"))
                    } finally {
                        val ms = (System.nanoTime() - t0) / 1_000_000
                        if (ms > 2_000) android.util.Log.w("TuraFs", "slow ${op.optString("op")} '${op.optString("path")}': $ms ms")
                    }
                }
            }
            client.onControl = { onControl(this@PcLink, it) }
            client.onBinary = onBinary@{ frame ->
                if (frame.isNotEmpty() && frame[0] == dev.tunnelrat.cast.VideoDecoder.KIND) {
                    if (this@PcLink === selectedLink) videoSink?.invoke(frame)
                    return@onBinary
                }
                val fs = currentFs()
                scope.launch(fsDispatcher) {
                    val w = dev.tunnelrat.drive.Bin.parseWrite(frame) ?: return@launch
                    if (fs == null) return@launch reply(this@PcLink, w.id, dev.tunnelrat.drive.fsError("denied"))
                    reply(this@PcLink, w.id, dev.tunnelrat.drive.writeOrError(fs, w))
                }
            }
            connect()
        }

        fun close() {
            jobs.forEach { it.cancel() }
            jobs.clear()
            client.onFsRequest = null
            client.onControl = null
            client.onBinary = null
            // OkHttp flushes queued frames before the close frame, so ForgetMe still goes out.
            client.disconnect()
        }
    }

    // ── Paired PCs ──

    private val _links = MutableStateFlow<List<PcLink>>(emptyList())
    val links: StateFlow<List<PcLink>> = _links.asStateFlow()
    private val _selected = MutableStateFlow(prefs.getString("selected_pc", null))

    private val selectedLink: PcLink?
        get() = _links.value.firstOrNull { it.fingerprint == _selected.value } ?: _links.value.firstOrNull()

    /** Placeholder while nothing is paired: never connects, sends nothing. */
    private val idleClient = TuraClient()

    /** The selected PC's connection: what the phone's own actions go to. */
    val client: TuraClient get() = selectedLink?.client ?: idleClient

    private val _pc = MutableStateFlow<PairedPc?>(null)
    /** The selected PC, or null → show the pairing screen. */
    val pc: StateFlow<PairedPc?> = _pc.asStateFlow()

    /** A scanned QR waiting for the user's "Pair" confirmation. */
    private val _offer = MutableStateFlow<PairOffer?>(null)
    val offer: StateFlow<PairOffer?> = _offer.asStateFlow()

    private fun refreshSelected() {
        val link = selectedLink
        _selected.value = link?.fingerprint
        prefs.edit().putString("selected_pc", link?.fingerprint).apply()
        _pc.value = link?.pc?.value
    }

    fun select(fingerprint: String) {
        if (_links.value.none { it.fingerprint == fingerprint }) return
        _selected.value = fingerprint
        refreshSelected()
    }

    private fun savePcs() {
        val arr = org.json.JSONArray()
        for (l in _links.value) {
            val p = l.pc.value
            arr.put(
                org.json.JSONObject().put("name", p.name).put("hosts", org.json.JSONArray(p.hosts)).put("port", p.port).put("fp", p.fingerprint),
            )
        }
        prefs.edit().putString("pcs", arr.toString()).apply()
    }

    /** Paired PCs from prefs (and the single PC of older versions). */
    private fun loadPcs(): List<PairedPc> {
        val json = prefs.getString("pcs", null)
        if (json == null) {
            val old = PairedPc.load(prefs) ?: return emptyList()
            prefs.getString("wake_mac", null)?.let { prefs.edit().putString("wake_mac_${old.fingerprint}", it).apply() }
            return listOf(old)
        }
        return runCatching {
            val arr = org.json.JSONArray(json)
            List(arr.length()) { i ->
                val o = arr.getJSONObject(i)
                val hosts = o.getJSONArray("hosts")
                PairedPc(o.getString("name"), List(hosts.length()) { hosts.getString(it) }, o.getInt("port"), o.getString("fp"))
            }
        }.getOrDefault(emptyList())
    }

    private fun addLink(pc: PairedPc, token: String?): PcLink {
        _links.value.firstOrNull { it.fingerprint == pc.fingerprint }?.let { removeLink(it) }
        val link = PcLink(pc, token)
        _links.value = _links.value + link
        link.start()
        savePcs()
        return link
    }

    private fun removeLink(link: PcLink) {
        link.close()
        _links.value = _links.value - link
        if (mirrorLink === link) stopMirror()
        savePcs()
        refreshSelected()
        TuraService.sync(app)
    }

    private val _toast = MutableStateFlow<String?>(null)
    val toast: StateFlow<String?> = _toast.asStateFlow()

    /** Phone's current route to the PC (see [Net]). Declared before init{}, which uses it. */
    private val _net = MutableStateFlow(Net.Local)
    val net: StateFlow<Net> = _net.asStateFlow()

    // ── Settings ──
    val syncClipboard = setting("sync_clipboard", true)
    val shareNotifications = setting("share_notifications", false)
    val keepConnected = setting("keep_connected", true)

    // ── Phone as drive ──
    enum class DriveMode { Off, Folder, All }
    private val _driveMode = MutableStateFlow(
        runCatching { DriveMode.valueOf(prefs.getString("drive_mode", "Off")!!) }.getOrDefault(DriveMode.Off),
    )
    val driveMode: StateFlow<DriveMode> = _driveMode.asStateFlow()
    private val _driveTree = MutableStateFlow(prefs.getString("drive_tree", null)?.let(Uri::parse))
    val driveTree: StateFlow<Uri?> = _driveTree.asStateFlow()
    /** File requests run here: a few in parallel, off the network thread. */
    private val fsDispatcher = Dispatchers.IO.limitedParallelism(4)

    private fun setting(key: String, default: Boolean) = MutableStateFlow(prefs.getBoolean(key, default))
    fun set(flow: MutableStateFlow<Boolean>, key: String, value: Boolean) {
        flow.value = value
        prefs.edit().putBoolean(key, value).apply()
    }

    private val clipboard = app.getSystemService(ClipboardManager::class.java)
    /** Text last synced either way, to avoid PC ↔ phone echo. */
    private var lastClip: String? = null
    /** Timestamp of the last phone clip we looked at (reading triggers Android's paste toast). */
    private var lastClipStamp = prefs.getLong("clip_stamp", 0L)

    private var battery: Message.Battery? = null
    private val fileIds = AtomicInteger((System.currentTimeMillis() % 100_000).toInt())

    /** Pointer speed for the touchpad and the cast view's trackpad mode: PC pixels per finger pixel. */
    var pointerSpeed: Float
        get() = prefs.getFloat("pointer_speed", DEFAULT_POINTER_SPEED)
        set(v) = prefs.edit().putFloat("pointer_speed", v.coerceIn(MIN_POINTER_SPEED, MAX_POINTER_SPEED)).apply()

    /** Gamepad mode: where the user put the buttons (JSON, see PadLayout). */
    var padLayout: dev.tunnelrat.gamepad.PadLayout
        get() = dev.tunnelrat.gamepad.PadLayout.fromJson(prefs.getString("pad_layout", null))
        set(v) = prefs.edit().putString("pad_layout", v.toJson()).apply()

    /** Gamepad mode: tilt the phone to steer the right stick. */
    var padGyro: Boolean
        get() = prefs.getBoolean("pad_gyro", false)
        set(v) = prefs.edit().putBoolean("pad_gyro", v).apply()

    /** Cast view: trackpad mode (true) or direct touch; remembered. */
    var castTrackpad: Boolean
        get() = prefs.getBoolean("cast_trackpad", true)
        set(v) = prefs.edit().putBoolean("cast_trackpad", v).apply()

    // ── Login at the PC's login screen ──

    private val _loggingIn = MutableStateFlow(false)
    val loggingIn: kotlinx.coroutines.flow.StateFlow<Boolean> = _loggingIn

    fun logIn(password: String) {
        if (password.isEmpty()) return
        _loggingIn.value = client.send(Message.Login(password))
    }

    // ── Mirroring (this phone's screen on the PC) ──

    private val _mirroring = MutableStateFlow(false)
    val mirroring: kotlinx.coroutines.flow.StateFlow<Boolean> = _mirroring
    @Volatile private var mirror: dev.tunnelrat.cast.ScreenMirror? = null
    /** The PC the phone's screen goes to (the one that asked, else the selected one). */
    @Volatile private var mirrorLink: PcLink? = null
    /** Set by a PC's request, used by the next [startMirror]. */
    @Volatile private var requestedBy: PcLink? = null
    private val main = android.os.Handler(android.os.Looper.getMainLooper())

    /** Called by [TuraService] (already a mediaProjection service) after the user agreed. */
    fun startMirror(code: Int, data: android.content.Intent) {
        mirror?.stop()
        val target = requestedBy?.takeIf { it in _links.value } ?: selectedLink ?: return TuraService.mirrorEnded(app)
        requestedBy = null
        val client = target.client
        val mpm = app.getSystemService(android.media.projection.MediaProjectionManager::class.java)
        val projection = mpm.getMediaProjection(code, data) ?: return TuraService.mirrorEnded(app)
        val m = try {
            dev.tunnelrat.cast.ScreenMirror(
                app, projection,
                send = { client.sendBytes(it) },
                queued = { client.queueSize() },
                onStopped = {
                    mirror = null
                    mirrorLink = null
                    _mirroring.value = false
                    client.send(Message.MirrorStop)
                    TuraService.mirrorEnded(app)
                },
            )
        } catch (e: Exception) {
            android.util.Log.e("TuraMirror", "can't start", e)
            projection.stop()
            _toast.value = "Can't show the screen: ${e.message}"
            return TuraService.mirrorEnded(app)
        }
        mirror = m
        mirrorLink = target
        _mirroring.value = true
        app.getSystemService(android.app.NotificationManager::class.java).cancel(MIRROR_REQUEST_NOTIF)
        client.send(Message.MirrorStart(m.width, m.height))
    }

    fun stopMirror() {
        mirror?.stop()
    }

    /**
     * The PC asked to see the screen: open the capture prompt. Android lets us
     * open it from the background while the control (accessibility) service is
     * on; a notification is the fallback. If PROJECT_MEDIA was allowed over adb,
     * Android approves it without asking.
     */
    private fun mirrorRequested(by: PcLink) {
        if (mirror != null) return
        requestedBy = by
        val open = android.content.Intent(app, MainActivity::class.java)
            .setAction(MainActivity.ACTION_MIRROR)
            .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK or android.content.Intent.FLAG_ACTIVITY_SINGLE_TOP)
        runCatching { app.startActivity(open) }
        val nm = app.getSystemService(android.app.NotificationManager::class.java)
        if (nm.getNotificationChannel(REQUESTS) == null) {
            nm.createNotificationChannel(android.app.NotificationChannel(REQUESTS, "Requests from the PC", android.app.NotificationManager.IMPORTANCE_HIGH))
        }
        val tap = android.app.PendingIntent.getActivity(app, 3, open, android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT)
        nm.notify(
            MIRROR_REQUEST_NOTIF,
            android.app.Notification.Builder(app, REQUESTS)
                .setSmallIcon(R.drawable.ic_tunnelrat)
                .setContentTitle("${by.pc.value.name} wants to show this phone's screen")
                .setContentText("Tap to start")
                .setContentIntent(tap)
                .setAutoCancel(true)
                .setTimeoutAfter(60_000)
                .build(),
        )
    }

    private fun onControl(from: PcLink, m: Message) {
        if (m is Message.MirrorStop) {
            if (from === mirrorLink) stopMirror()
            return
        }
        if (m is Message.Ring) {
            Ringer.start(app, from.pc.value.name)
            return
        }
        if (m is Message.MirrorRequest) {
            main.post { mirrorRequested(from) }
            return
        }
        if (m is Message.PhoneReply) {
            main.post { _toast.value = TuraNotificationListener.reply(app, m.key, m.text) }
            return
        }
        // Only the PC the screen is shown on, and only while mirroring.
        if (mirror == null || from !== mirrorLink) return
        val svc = TuraControlService.instance
        if (svc == null) {
            _toast.value = "To control the phone from the PC, turn on “Tunnelrat: control from PC” in Accessibility settings"
            return
        }
        main.post { svc.handle(m) }
    }

    /** Screen cast frames go here while the cast view is open (OkHttp's reader thread). */
    @Volatile var videoSink: ((ByteArray) -> Unit)? = null

    // ── Finding the PC on the local network (its address may change) ──

    private val discovery = dev.tunnelrat.net.Discovery(app) { fp, host, port -> onDiscovered(fp, host, port) }

    /** Look for PCs while on Wi-Fi and some paired PC isn't connected; main thread. */
    private fun updateDiscovery() {
        val want = _links.value.any { !it.isConnected } && _net.value == Net.Local
        if (want) discovery.start() else discovery.stop()
    }

    private fun onDiscovered(fp: String, host: String, port: Int) {
        val link = _links.value.firstOrNull { it.fingerprint.lowercase() == fp } ?: return // not ours
        val pc = link.pc.value
        if (pc.hosts.firstOrNull() != host || pc.port != port) {
            android.util.Log.i("TuraDiscovery", "${pc.name} is at $host:$port")
            link.pc.value = pc.copy(hosts = (listOf(host) + pc.hosts.filter { it != host }).take(6), port = port)
            savePcs()
            refreshSelected()
        }
        link.hostIdx = 0
        val st = link.client.state.value
        if (st !is TuraClient.State.Connected && st !is TuraClient.State.Connecting) link.connect()
    }

    init {
        for (p in loadPcs()) addLink(p, null)
        refreshSelected()
        watchBattery()
        watchNetwork()
    }

    // ── Bluetooth pairing, for the fallback link ──

    /** Settings shows this while a Bluetooth setup runs. */
    private val _btSetupBusy = MutableStateFlow(false)
    val btSetupBusy: StateFlow<Boolean> = _btSetupBusy.asStateFlow()
    private var btPairing: PcLink? = null

    private fun btAdapter() = app.getSystemService(android.bluetooth.BluetoothManager::class.java)?.adapter

    /** The selected PC's Bluetooth address, once it told us (it has Bluetooth). */
    val selectedBtAddress: String? get() = selectedLink?.btAddress

    /** The phone is Bluetooth-paired with the selected PC. */
    @android.annotation.SuppressLint("MissingPermission")
    fun selectedBtPaired(): Boolean {
        val addr = selectedBtAddress ?: return false
        if (!dev.tunnelrat.net.BtBridge.permitted(app)) return false
        return runCatching { btAdapter()?.bondedDevices?.any { it.address.equals(addr, ignoreCase = true) } == true }.getOrDefault(false)
    }

    /**
     * Pairs the phone with the selected PC over Bluetooth: asks the PC (over
     * the network link, which proves it's us) to accept this phone for a
     * minute, then bonds. Android shows its own "Pair?" prompt.
     */
    /** Shown under the setup button: why it can't start, or how it ended. */
    private val _btSetupNote = MutableStateFlow<String?>(null)
    val btSetupNote: StateFlow<String?> = _btSetupNote.asStateFlow()

    @android.annotation.SuppressLint("MissingPermission")
    fun setUpBluetooth() {
        val link = selectedLink ?: return
        val name = link.pc.value.name
        val adapter = btAdapter()
        val problem = when {
            !dev.tunnelrat.net.BtBridge.permitted(app) -> "Allow “Nearby devices” for Tunnelrat first"
            adapter == null -> "This phone has no Bluetooth"
            !adapter.isEnabled -> "Turn on Bluetooth first"
            !link.isConnected || link.client.viaBluetooth -> "Connect to $name over Wi-Fi first"
            link.btAddress == null -> "$name has no Bluetooth, or it's switched off"
            selectedBtPaired() -> "Already paired with $name"
            else -> null
        }
        android.util.Log.i("TuraBt", "set up Bluetooth with $name: ${problem ?: "asking the PC"}")
        _btSetupNote.value = problem
        if (problem != null) return
        btPairing = link
        _btSetupBusy.value = true
        if (!link.client.send(Message.BtPairStart(adapter!!.name ?: android.os.Build.MODEL))) {
            btPairing = null
            _btSetupBusy.value = false
            _btSetupNote.value = "Lost the connection to $name; try again"
        }
    }

    @android.annotation.SuppressLint("MissingPermission")
    private fun onBtPair(link: PcLink, result: Message.BtPairResult?) {
        if (btPairing !== link) return
        val name = link.pc.value.name
        if (result == null) {
            // The PC is listening: bond (Android asks the user to confirm).
            val ok = runCatching { btAdapter()?.getRemoteDevice(link.btAddress!!.uppercase())?.createBond() == true }.getOrDefault(false)
            android.util.Log.i("TuraBt", "$name is ready; createBond=$ok")
            if (!ok) _btSetupNote.value = "Couldn't start Bluetooth pairing with $name"
            return
        }
        btPairing = null
        _btSetupBusy.value = false
        android.util.Log.i("TuraBt", "pairing with $name: ok=${result.ok} ${result.message ?: ""}")
        _btSetupNote.value = if (result.ok) null else "Pairing failed: ${result.message ?: "unknown reason"}".ellipsize(160)
        if (result.ok) _toast.value = "Bluetooth ready: $name stays reachable without Wi-Fi"
    }

    private fun onEvent(link: PcLink, ev: TuraClient.Event) {
        val name = link.pc.value.name
        when (ev) {
            is TuraClient.Event.Paired -> {
                link.pendingToken = null
                _toast.value = "Paired with ${ev.pcName}"
            }
            is TuraClient.Event.Rejected -> {
                // Token bad/expired, or the PC forgot us: drop this pairing.
                link.pendingToken = null
                removeLink(link)
                _toast.value = "$name: ${ev.reason}".ellipsize(160)
            }
            is TuraClient.Event.ServerError -> _toast.value = (if (_links.value.size > 1) "$name: ${ev.message}" else ev.message).ellipsize(160)
            is TuraClient.Event.Clipboard -> if (clipboardPeer(link)) onPcClipboard(ev.text)
            is TuraClient.Event.FileSaved -> fileSaved(ev.path)
            is TuraClient.Event.BtPair -> onBtPair(link, ev.result)
            is TuraClient.Event.LoginResult -> {
                _loggingIn.value = false
                _toast.value = if (ev.ok) ev.message ?: "Logged in" else ev.message ?: "Login failed"
            }
        }
    }

    private fun handleFsRequest(link: PcLink, fs: dev.tunnelrat.drive.PhoneFs?, id: Long, op: org.json.JSONObject) {
        if (fs == null) return reply(link, id, dev.tunnelrat.drive.fsError("denied"))
        if (op.optString("op") == "read") {
            // Read data goes back as a binary frame: no JSON/base64 (slow on Android).
            val data = dev.tunnelrat.drive.readOrError(fs, op)
            data.onSuccess { link.client.sendBytes(dev.tunnelrat.drive.Bin.readData(id, it)) }
                .onFailure { reply(link, id, dev.tunnelrat.drive.fsError((it as? dev.tunnelrat.drive.FsException)?.kind ?: "io")) }
        } else {
            reply(link, id, dev.tunnelrat.drive.handleFsOp(fs, op))
        }
    }

    // ── Phone network ──

    /** What the phone's current (default) network can reach. */
    enum class Net(val canReachPc: Boolean) {
        /** Wi-Fi or Ethernet: the PC's LAN address may work. */
        Local(true),
        /** A VPN such as Tailscale: the PC's tailnet address may work. */
        Vpn(true),
        /** Mobile data only: the home LAN is unreachable. */
        MobileOnly(false),
        None(false),
    }

    private fun classify(c: android.net.NetworkCapabilities): Net = when {
        c.hasTransport(android.net.NetworkCapabilities.TRANSPORT_VPN) -> Net.Vpn
        c.hasTransport(android.net.NetworkCapabilities.TRANSPORT_WIFI) ||
            c.hasTransport(android.net.NetworkCapabilities.TRANSPORT_ETHERNET) -> Net.Local
        else -> Net.MobileOnly
    }

    private fun watchNetwork() {
        val cm = app.getSystemService(android.net.ConnectivityManager::class.java)
        cm.activeNetwork?.let(cm::getNetworkCapabilities)?.let { _net.value = classify(it) } ?: run { _net.value = Net.None }
        cm.registerDefaultNetworkCallback(
            object : android.net.ConnectivityManager.NetworkCallback() {
                override fun onCapabilitiesChanged(n: android.net.Network, caps: android.net.NetworkCapabilities) = onNet(classify(caps))
                override fun onLost(n: android.net.Network) = onNet(Net.None)
            },
            android.os.Handler(android.os.Looper.getMainLooper()),
        )
    }

    private fun onNet(new: Net) {
        val old = _net.value
        _net.value = new
        main.post { updateDiscovery() }
        // A new route home: reconnect now (LAN address first) instead of waiting out the backoff.
        if (new != old && new.canReachPc) {
            for (l in _links.value) l.maybeLeaveBluetooth(force = true)
            for (l in _links.value) if (!l.isConnected) {
                l.hostIdx = 0
                l.connect()
            }
        }
    }

    /** The selected PC is connected. */
    val isConnected get() = selectedLink?.isConnected == true

    /** For the tile/widget: wait briefly for the selected PC, then send. */
    suspend fun sendWhenConnected(m: Message, timeoutMs: Long = 8_000): Boolean {
        val link = selectedLink ?: return false
        if (!link.isConnected) link.connect()
        val ok = withTimeoutOrNull(timeoutMs) { link.client.state.first { it is TuraClient.State.Connected } } != null
        return ok && link.client.send(m)
    }

    // ── Pairing ──

    fun onPairLink(uri: Uri) {
        val offer = parsePairUri(uri)
        if (offer == null) _toast.value = "That isn't a valid Tunnelrat pairing code" else _offer.value = offer
    }

    /** Pairs another PC (or re-pairs a known one) and selects it. */
    fun acceptOffer() {
        val offer = _offer.value ?: return
        _offer.value = null
        addLink(offer.pc, offer.token)
        select(offer.pc.fingerprint)
        TuraService.sync(app)
    }

    fun declineOffer() { _offer.value = null }

    /** User tapped "Unpair": tell the selected PC to forget us too, then forget it. */
    fun unpair() {
        val link = selectedLink ?: return
        val told = link.client.send(Message.ForgetMe)
        val name = link.pc.value.name
        removeLink(link)
        PairedPc.forget(prefs) // the single-PC keys of older versions
        _toast.value = if (told) "Unpaired from $name on both devices" else "Unpaired here. Run `tura unpair` on $name too."
    }

    // ── Power ──

    fun power(action: PowerAction) {
        if (!client.send(Message.Power(action))) _toast.value = "Not connected"
    }

    fun wake(link: PcLink? = selectedLink, onDone: (String) -> Unit = { _toast.value = it }) {
        val mac = link?.wakeMac ?: return onDone("Connect once so Tunnelrat learns the PC's MAC")
        scope.launch {
            runCatching { WakeOnLan.send(mac) }
                .onSuccess { onDone("Wake signal sent") }
                .onFailure { onDone("Wake failed: ${it.message}") }
        }
    }

    // ── Phone as drive ──

    private fun reply(link: PcLink, id: Long, result: org.json.JSONObject) {
        link.client.send(Message.FsReply(id, result))
    }

    fun hasAllFilesAccess() = android.os.Environment.isExternalStorageManager()

    /** One backend per (mode, folder): SafFs keeps listing/id caches that must survive between requests. */
    private var fsKey: Pair<DriveMode, Uri?>? = null
    private var fsInstance: dev.tunnelrat.drive.PhoneFs? = null

    @Synchronized
    private fun currentFs(): dev.tunnelrat.drive.PhoneFs? {
        val allowed = _driveMode.value != DriveMode.All || hasAllFilesAccess()
        val key = _driveMode.value to _driveTree.value
        if (key != fsKey || (fsInstance == null && allowed)) {
            fsKey = key
            fsInstance = when (_driveMode.value) {
                DriveMode.Off -> null
                DriveMode.All -> dev.tunnelrat.drive.FileFs(android.os.Environment.getExternalStorageDirectory())
                DriveMode.Folder -> _driveTree.value?.let { dev.tunnelrat.drive.SafFs(app, it) }
            }
        }
        return if (allowed) fsInstance else null
    }

    /** Name of the picked folder, e.g. "Download". */
    fun folderName(tree: Uri): String = runCatching {
        val id = android.provider.DocumentsContract.getTreeDocumentId(tree)
        app.contentResolver.query(
            android.provider.DocumentsContract.buildDocumentUriUsingTree(tree, id),
            arrayOf(android.provider.DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null,
        )?.use { if (it.moveToFirst()) it.getString(0) else null }
    }.getOrNull() ?: "Folder"

    /** Tells the PC(s) to mount (or unmount) according to the current setting. */
    private fun offerDrive(only: PcLink? = null) {
        val label = when {
            currentFs() == null -> null
            _driveMode.value == DriveMode.Folder -> "${Build.MODEL} · ${folderName(_driveTree.value!!)}"
            else -> Build.MODEL
        }
        for (l in only?.let(::listOf) ?: _links.value) l.client.send(Message.DriveOffer(label))
    }

    fun setDriveMode(mode: DriveMode) {
        _driveMode.value = mode
        prefs.edit().putString("drive_mode", mode.name).apply()
        offerDrive()
    }

    /** A folder picked with the system picker; keeps access across reboots. */
    fun setDriveFolder(tree: Uri) {
        val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        app.contentResolver.takePersistableUriPermission(tree, flags)
        _driveTree.value?.takeIf { it != tree }?.let { old ->
            runCatching { app.contentResolver.releasePersistableUriPermission(old, flags) }
        }
        _driveTree.value = tree
        prefs.edit().putString("drive_tree", tree.toString()).apply()
        setDriveMode(DriveMode.Folder)
    }

    /** Re-check after returning from Android's "All files access" screen. */
    fun refreshDrive() = offerDrive()

    // ── Battery ──

    private fun watchBattery() {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(c: Context, i: Intent) {
                val level = i.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
                val scale = i.getIntExtra(BatteryManager.EXTRA_SCALE, 100)
                if (level < 0 || scale <= 0) return
                val status = i.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
                val charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL
                val b = Message.Battery(level * 100 / scale, charging)
                if (b != battery) {
                    battery = b
                    for (l in _links.value) l.client.send(b)
                }
            }
        }
        // Sticky: delivers the current state immediately, then every change.
        app.registerReceiver(receiver, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
    }

    // ── Clipboard ──

    /** Clipboard syncs with the selected PC, and with the PC the phone is shown on. */
    private fun clipboardPeer(link: PcLink) = link === selectedLink || link === mirrorLink

    private fun onPcClipboard(text: String) {
        if (!syncClipboard.value || text == lastClip) return
        lastClip = text
        val clip = ClipData.newPlainText("From PC", text)
        clipboard.setPrimaryClip(clip)
        // Our own write must not bounce back on the next resume.
        lastClipStamp = clipboard.primaryClipDescription?.timestamp ?: lastClipStamp
        prefs.edit().putLong("clip_stamp", lastClipStamp).apply()
    }

    /**
     * Phone → PC. Android only lets the foreground app read the clipboard, so
     * this is called when an activity gains focus. Only reads (which shows
     * Android's "pasted" toast) when the clip is new, and skips clips marked
     * sensitive (passwords from password managers).
     */
    fun pushClipboardIfNew(force: Boolean = false): String? {
        val r = readAndSendClipboard(force)
        android.util.Log.i("Tunnelrat", "clipboard push (force=$force): ${r ?: "nothing new"}")
        return r
    }

    private fun readAndSendClipboard(force: Boolean): String? {
        if (!syncClipboard.value && !force) return null
        // Null both when empty and when Android denies access (we're not focused yet).
        val desc = clipboard.primaryClipDescription ?: return "Couldn't read the clipboard"
        if (!force && desc.timestamp == lastClipStamp) return null
        if (isSensitive(desc)) {
            remember(desc.timestamp)
            return "Not sent: the clipboard is marked sensitive"
        }
        val text = clipboard.primaryClip?.getItemAt(0)?.coerceToText(app)?.toString()
        if (text.isNullOrEmpty()) return "The clipboard has no text"
        if (text.length > 256 * 1024) {
            remember(desc.timestamp)
            return "Clipboard text is too large"
        }
        if (text == lastClip && !force) {
            remember(desc.timestamp)
            return null
        }
        // Only mark it handled once it actually went out, so a clip copied
        // while disconnected is retried on the next focus.
        val peers = _links.value.filter(::clipboardPeer)
        if (peers.count { it.client.send(Message.Clipboard(text)) } == 0) return "Not connected"
        lastClip = text
        remember(desc.timestamp)
        return "Clipboard sent to PC"
    }

    private fun remember(stamp: Long) {
        lastClipStamp = stamp
        prefs.edit().putLong("clip_stamp", stamp).apply()
    }

    private fun isSensitive(desc: ClipDescription): Boolean {
        val extras: PersistableBundle = desc.extras ?: return false
        val key = if (Build.VERSION.SDK_INT >= 33) ClipDescription.EXTRA_IS_SENSITIVE else "android.content.extra.IS_SENSITIVE"
        return extras.getBoolean(key, false)
    }

    // ── Share to PC ──

    fun shareText(text: String, onDone: (String) -> Unit = { _toast.value = it }) {
        val t = text.trim()
        val url = Regex("""https?://\S+""").find(t)?.value
        val msg = if (url != null && url.length >= t.length - 1) Message.OpenUrl(url) else Message.Clipboard(t).also { lastClip = t }
        scope.launch {
            val ok = sendWhenConnected(msg)
            onDone(when {
                !ok -> "PC not reachable"
                msg is Message.OpenUrl -> "Opened on PC"
                else -> "Copied to PC clipboard"
            })
        }
    }

    /**
     * A shared file opened *by the receiving activity*: Android's read grant
     * belongs to that activity, so we open it there and keep the descriptor
     * (still readable after the activity finishes).
     */
    class OpenedFile(val name: String, val fd: android.os.ParcelFileDescriptor)

    fun openShared(uri: Uri): OpenedFile? = runCatching {
        val (name, _) = describe(uri)
        app.contentResolver.openFileDescriptor(uri, "r")?.let { OpenedFile(name, it) }
    }.getOrNull()

    /** Streams an opened file to the PC in base64 chunks, with back-pressure. */
    fun shareFile(file: OpenedFile, onDone: (String) -> Unit = { _toast.value = it }) {
        val name = file.name
        scope.launch {
            // Pipes/sockets report no size; spool those to cache so we can announce one.
            val spooled: java.io.File? = if (file.fd.statSize >= 0) null else withContext(Dispatchers.IO) {
                runCatching {
                    val f = java.io.File.createTempFile("share", ".bin", app.cacheDir)
                    android.os.ParcelFileDescriptor.AutoCloseInputStream(file.fd).use { i -> f.outputStream().use { o -> i.copyTo(o) } }
                    f
                }.getOrNull()
            }
            if (file.fd.statSize < 0 && spooled == null) return@launch onDone("Couldn't read $name")
            val size = spooled?.length() ?: file.fd.statSize
            try {
                if (!sendWhenConnected(Message.Ping)) return@launch onDone("PC not reachable")
                val id = fileIds.incrementAndGet()
                client.send(Message.FileStart(id, name, size))
                val result = withContext(Dispatchers.IO) {
                    runCatching {
                        val input = spooled?.inputStream() ?: android.os.ParcelFileDescriptor.AutoCloseInputStream(file.fd)
                        input.use {
                            val buf = ByteArray(FILE_CHUNK)
                            var total = 0L
                            while (true) {
                                val n = it.readNBytesCompat(buf)
                                if (n <= 0) break
                                while (client.queueSize() > 4L * 1024 * 1024) delay(20)
                                if (!client.send(Message.FileChunk(id, Base64.encodeToString(buf, 0, n, Base64.NO_WRAP)))) {
                                    error("connection lost")
                                }
                                total += n
                            }
                            check(total == size) { "file changed while sending" }
                        }
                    }
                }
                result.onSuccess { client.send(Message.FileEnd(id)); onDone("Sending $name…") }
                    .onFailure { onDone("Couldn't send $name: ${it.message}") }
            } finally {
                spooled?.delete()
                runCatching { file.fd.close() }
            }
        }
    }

    private fun describe(uri: Uri): Pair<String, Long> {
        app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
            if (c.moveToFirst()) {
                val name = c.getString(0) ?: "shared-file"
                val size = if (c.isNull(1)) -1L else c.getLong(1)
                return name to size
            }
        }
        return (uri.lastPathSegment ?: "shared-file") to -1L
    }

    // ── Notifications (from TuraNotificationListener) ──

    fun forwardNotification(m: Message) {
        if (shareNotifications.value) for (l in _links.value) l.client.send(m)
    }

    /** Also shown when the app UI is closed (share sheet use). */
    private fun fileSaved(path: String) {
        android.widget.Toast.makeText(app, "Saved on PC: ${path.substringAfterLast('/')}", android.widget.Toast.LENGTH_SHORT).show()
    }

    fun toastShown() { _toast.value = null }
    fun notify(msg: String) { _toast.value = msg }
}

/** Reads until [buf] is full or EOF; returns bytes read (0 at EOF). */
private fun java.io.InputStream.readNBytesCompat(buf: ByteArray): Int {
    var off = 0
    while (off < buf.size) {
        val n = read(buf, off, buf.size - off)
        if (n < 0) break
        off += n
    }
    return off
}

/** Keeps snackbars readable even if the PC sends a long error. */
internal fun String.ellipsize(max: Int) = if (length <= max) this else take(max - 1).trimEnd() + "…"

const val DEFAULT_POINTER_SPEED = 1.4f
const val MIN_POINTER_SPEED = 0.4f
const val MAX_POINTER_SPEED = 3.5f
