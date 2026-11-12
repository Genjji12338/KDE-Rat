package dev.tunnelrat.net

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress

object WakeOnLan {
    /** Sends the magic packet (6×0xFF + 16×MAC) to UDP port 9 on the broadcast address. */
    suspend fun send(mac: String, broadcast: String = "255.255.255.255") = withContext(Dispatchers.IO) {
        val macBytes = mac.split(':', '-').map { it.toInt(16).toByte() }
        require(macBytes.size == 6) { "MAC must have 6 bytes: $mac" }
        val packet = ByteArray(6) { 0xFF.toByte() } + List(16) { macBytes }.flatten().toByteArray()
        DatagramSocket().use { sock ->
            sock.broadcast = true
            sock.send(DatagramPacket(packet, packet.size, InetAddress.getByName(broadcast), 9))
        }
    }
}
