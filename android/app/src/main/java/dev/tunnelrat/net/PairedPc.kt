package dev.tunnelrat.net

import android.content.SharedPreferences
import android.net.Uri

/** What the QR code tells us about a PC. */
data class PairedPc(
    val name: String,
    val hosts: List<String>,
    val port: Int,
    /** SHA-256 hex of the PC's TLS certificate. */
    val fingerprint: String,
) {
    fun save(p: SharedPreferences) = p.edit()
        .putString("pc_name", name)
        .putString("pc_hosts", hosts.joinToString(","))
        .putInt("pc_port", port)
        .putString("pc_fp", fingerprint)
        .apply()

    companion object {
        fun load(p: SharedPreferences): PairedPc? {
            val fp = p.getString("pc_fp", null) ?: return null
            return PairedPc(
                name = p.getString("pc_name", "PC")!!,
                hosts = p.getString("pc_hosts", "")!!.split(',').filter { it.isNotBlank() },
                port = p.getInt("pc_port", dev.tunnelrat.proto.DEFAULT_PORT),
                fingerprint = fp,
            )
        }

        fun forget(p: SharedPreferences) =
            p.edit().remove("pc_name").remove("pc_hosts").remove("pc_port").remove("pc_fp").apply()
    }
}

data class PairOffer(val pc: PairedPc, val token: String)

/**
 * Parses `tunnelrat://pair?v=1&n=<name>&h=<ip,ip>&p=<port>&fp=<sha256>&t=<token>`.
 * Returns null for anything malformed so a bad link can't half-pair.
 */
fun parsePairUri(uri: Uri): PairOffer? {
    if (uri.scheme != "tunnelrat" || uri.host != "pair" || uri.getQueryParameter("v") != "1") return null
    val fp = uri.getQueryParameter("fp")?.lowercase()?.takeIf { it.matches(Regex("[0-9a-f]{64}")) } ?: return null
    val token = uri.getQueryParameter("t")?.takeIf { it.matches(Regex("[0-9a-f]{32}")) } ?: return null
    val hosts = uri.getQueryParameter("h")?.split(',')?.map { it.trim() }
        ?.filter { it.matches(Regex("""\d{1,3}(\.\d{1,3}){3}""")) }?.takeIf { it.isNotEmpty() } ?: return null
    val port = uri.getQueryParameter("p")?.toIntOrNull()?.takeIf { it in 1..65535 } ?: return null
    val name = uri.getQueryParameter("n")?.take(64)?.ifBlank { null } ?: "PC"
    return PairOffer(PairedPc(name, hosts, port, fp), token)
}
