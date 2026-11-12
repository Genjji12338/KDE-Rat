package dev.tunnelrat.net

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.util.Log
import java.net.Inet4Address

/**
 * Finds Tunnelrat PCs on the local network (mDNS / DNS-SD, `_tunnelrat._tcp`,
 * announced by turad through Avahi). Reports (fingerprint, address, port);
 * the caller only trusts the paired fingerprint, and TLS pinning still
 * decides who the PC really is.
 */
class Discovery(context: Context, private val onFound: (fingerprint: String, host: String, port: Int) -> Unit) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val executor = context.mainExecutor
    private var listener: NsdManager.DiscoveryListener? = null

    val running get() = listener != null

    fun start() {
        if (listener != null) return
        val l = object : NsdManager.DiscoveryListener {
            override fun onServiceFound(info: NsdServiceInfo) = resolve(info)
            override fun onServiceLost(info: NsdServiceInfo) {}
            override fun onDiscoveryStarted(type: String) {}
            override fun onDiscoveryStopped(type: String) {}
            override fun onStartDiscoveryFailed(type: String, code: Int) {
                Log.w(TAG, "discovery failed: $code")
                listener = null
            }
            override fun onStopDiscoveryFailed(type: String, code: Int) {}
        }
        listener = l
        runCatching { nsd.discoverServices(SERVICE, NsdManager.PROTOCOL_DNS_SD, l) }.onFailure {
            Log.w(TAG, "discovery: $it")
            listener = null
        }
    }

    fun stop() {
        val l = listener ?: return
        listener = null
        runCatching { nsd.stopServiceDiscovery(l) }
    }

    private fun resolve(info: NsdServiceInfo) {
        if (Build.VERSION.SDK_INT >= 34) {
            val cb = object : NsdManager.ServiceInfoCallback {
                override fun onServiceUpdated(si: NsdServiceInfo) {
                    report(si)
                    runCatching { nsd.unregisterServiceInfoCallback(this) }
                }
                override fun onServiceInfoCallbackRegistrationFailed(code: Int) {}
                override fun onServiceLost() {}
                override fun onServiceInfoCallbackUnregistered() {}
            }
            runCatching { nsd.registerServiceInfoCallback(info, executor, cb) }
        } else {
            @Suppress("DEPRECATION")
            nsd.resolveService(
                info,
                object : NsdManager.ResolveListener {
                    override fun onServiceResolved(si: NsdServiceInfo) = report(si)
                    override fun onResolveFailed(si: NsdServiceInfo, code: Int) {}
                },
            )
        }
    }

    private fun report(si: NsdServiceInfo) {
        val fp = si.attributes["fp"]?.toString(Charsets.UTF_8)?.lowercase() ?: return
        @Suppress("DEPRECATION")
        val addrs = if (Build.VERSION.SDK_INT >= 34) si.hostAddresses else listOfNotNull(si.host)
        // IPv4 on the LAN; the pairing code's addresses are IPv4 too.
        val host = addrs.firstOrNull { it is Inet4Address && it.isSiteLocalAddress } ?: addrs.firstOrNull { it is Inet4Address } ?: return
        executor.execute { onFound(fp, host.hostAddress ?: return@execute, si.port) }
    }

    companion object {
        private const val TAG = "TuraDiscovery"
        private const val SERVICE = "_tunnelrat._tcp"
    }
}
