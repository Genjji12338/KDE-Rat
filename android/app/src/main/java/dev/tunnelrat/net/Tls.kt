package dev.tunnelrat.net

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.net.Socket
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.Principal
import java.security.PrivateKey
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import java.util.Calendar
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509TrustManager
import javax.security.auth.x500.X500Principal

/**
 * This phone's TLS client identity: an EC P-256 key that never leaves the
 * Android Keystore (hardware-backed on Pixels), with the self-signed
 * certificate the Keystore creates for it. turad trusts it by fingerprint.
 */
object PhoneIdentity {
    private const val ALIAS = "tunnelrat-client"
    private val ks: KeyStore by lazy { KeyStore.getInstance("AndroidKeyStore").apply { load(null) } }

    @Synchronized
    fun ensure() {
        if (ks.containsAlias(ALIAS)) return
        val until = Calendar.getInstance().apply { add(Calendar.YEAR, 30) }.time
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            // TLS stacks may hand the Keystore a raw digest (NONE) or ask it to hash.
            .setDigests(KeyProperties.DIGEST_NONE, KeyProperties.DIGEST_SHA256, KeyProperties.DIGEST_SHA384, KeyProperties.DIGEST_SHA512)
            .setCertificateSubject(X500Principal("CN=Tunnelrat phone"))
            .setCertificateNotAfter(until)
            .build()
        KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").run {
            initialize(spec)
            generateKeyPair()
        }
    }

    fun privateKey(): PrivateKey = ks.getKey(ALIAS, null) as PrivateKey
    fun chain(): Array<X509Certificate> = ks.getCertificateChain(ALIAS).map { it as X509Certificate }.toTypedArray()

    val keyManager = object : X509ExtendedKeyManager() {
        override fun chooseClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?) = ALIAS
        override fun chooseEngineClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, engine: SSLEngine?) = ALIAS
        override fun getCertificateChain(alias: String?) = chain()
        override fun getPrivateKey(alias: String?) = privateKey()
        override fun getClientAliases(keyType: String?, issuers: Array<out Principal>?) = arrayOf(ALIAS)
        override fun getServerAliases(keyType: String?, issuers: Array<out Principal>?) = null
        override fun chooseServerAlias(keyType: String?, issuers: Array<out Principal>?, socket: Socket?) = null
    }
}

fun sha256Hex(bytes: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

/** Accepts exactly one server certificate: the one whose SHA-256 was in the QR code. */
class PinnedTrustManager(private val pin: String) : X509TrustManager {
    override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String?) {
        val got = sha256Hex(chain.firstOrNull()?.encoded ?: throw CertificateException("no server certificate"))
        if (!got.equals(pin, ignoreCase = true)) throw CertificateException("PC certificate doesn't match the paired one")
    }
    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) =
        throw CertificateException("client mode only")
    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}

fun pinnedSslContext(trust: X509TrustManager): SSLContext {
    PhoneIdentity.ensure()
    return SSLContext.getInstance("TLS").apply { init(arrayOf(PhoneIdentity.keyManager), arrayOf(trust), null) }
}
