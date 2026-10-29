package sh.zuse.android

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.math.BigInteger
import java.net.URI
import java.security.*
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.util.UUID
import sh.zuse.core.*

class DeviceProof {
    private val alias = "zuse.native.device"
    private val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    init {
        if (!store.containsAlias(alias))
            KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
                .apply {
                    initialize(
                        KeyGenParameterSpec.Builder(
                                alias,
                                KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY,
                            )
                            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                            .setDigests(KeyProperties.DIGEST_SHA256)
                            .build()
                    )
                }
                .generateKeyPair()
    }

    fun sign(method: String, address: String, accessToken: String?): String {
        val uri = URI(address)
        val target = URI(uri.scheme, null, uri.host, uri.port, uri.path, null, null).toString()
        val publicKey = store.getCertificate(alias).publicKey as ECPublicKey
        fun coordinate(value: BigInteger) =
            value
                .toByteArray()
                .takeLast(32)
                .toByteArray()
                .let { ByteArray(32 - it.size) + it }
                .base64Url()
        val header =
            obj(
                "alg" to "ES256".json(),
                "typ" to "dpop+jwt".json(),
                "jwk" to
                    obj(
                        "kty" to "EC".json(),
                        "crv" to "P-256".json(),
                        "x" to coordinate(publicKey.w.affineX).json(),
                        "y" to coordinate(publicKey.w.affineY).json(),
                    ),
            )
        val payload =
            obj(
                "htm" to method.uppercase().json(),
                "htu" to target.json(),
                "iat" to (System.currentTimeMillis() / 1000).json(),
                "jti" to UUID.randomUUID().toString().json(),
                "ath" to
                    accessToken?.let {
                        MessageDigest.getInstance("SHA-256")
                            .digest(it.toByteArray())
                            .base64Url()
                            .json()
                    },
            )
        val input =
            header.toString().toByteArray().base64Url() +
                "." +
                payload.toString().toByteArray().base64Url()
        val signature =
            Signature.getInstance("SHA256withECDSA")
                .apply {
                    initSign(store.getKey(alias, null) as PrivateKey)
                    update(input.toByteArray())
                }
                .sign()
        // Android returns ASN.1 DER; JOSE requires fixed-width unsigned r || s.
        var offset = 2
        require(signature[offset++].toInt() == 2)
        val rLength = signature[offset++].toInt() and 255
        val r = signature.copyOfRange(offset, offset + rLength)
        offset += rLength
        require(signature[offset++].toInt() == 2)
        val sLength = signature[offset++].toInt() and 255
        val s = signature.copyOfRange(offset, offset + sLength)
        fun fixed(value: ByteArray): ByteArray {
            val trimmed = value.takeLast(32).toByteArray()
            return ByteArray(32 - trimmed.size) + trimmed
        }
        return "$input.${(fixed(r) + fixed(s)).base64Url()}"
    }
}
