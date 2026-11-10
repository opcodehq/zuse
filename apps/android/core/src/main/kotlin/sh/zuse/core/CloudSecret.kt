package sh.zuse.core

import java.math.BigInteger
import java.security.KeyFactory
import java.security.spec.MGF1ParameterSpec
import java.security.spec.RSAPublicKeySpec
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.spec.OAEPParameterSpec
import javax.crypto.spec.PSource

/** Control plane receives only RSA-OAEP-SHA256 ciphertext, sealed to the auth authority. */
object CloudSecret {
    fun seal(secret: String, publicJwk: String): String {
        val jwk = WireJson.parseToJsonElement(publicJwk)
        require(jwk.text("kty") == "RSA") { "Unsupported cloud encryption key." }
        val n = Base64.getUrlDecoder().decode(jwk.text("n") ?: error("Missing RSA modulus."))
        val e = Base64.getUrlDecoder().decode(jwk.text("e") ?: error("Missing RSA exponent."))
        require(n.size >= 256) { "Invalid cloud encryption key." }
        val key =
            KeyFactory.getInstance("RSA")
                .generatePublic(RSAPublicKeySpec(BigInteger(1, n), BigInteger(1, e)))
        val cipher = Cipher.getInstance("RSA/ECB/OAEPPadding")
        cipher.init(
            Cipher.ENCRYPT_MODE,
            key,
            OAEPParameterSpec(
                "SHA-256",
                "MGF1",
                MGF1ParameterSpec.SHA256,
                PSource.PSpecified.DEFAULT,
            ),
        )
        return Base64.getUrlEncoder()
            .withoutPadding()
            .encodeToString(cipher.doFinal(secret.toByteArray()))
    }
}
