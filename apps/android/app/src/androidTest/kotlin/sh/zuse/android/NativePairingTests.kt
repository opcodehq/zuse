package sh.zuse.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import java.util.Base64
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import sh.zuse.core.*

@RunWith(AndroidJUnit4::class)
class NativePairingTests {
    @Test
    fun deviceProofBindsAccessTokenAndKeepsQueryOutOfTarget() {
        val proof = DeviceProof()
        val signed =
            proof.sign("POST", "https://host.example/v1/connect?secret=query", "access-token")
        val fields = signed.split('.')
        val claims = WireJson.parseToJsonElement(String(Base64.getUrlDecoder().decode(fields[1])))
        assertEquals("https://host.example/v1/connect", claims.text("htu"))
        assertEquals("POST", claims.text("htm"))
        val expected =
            java.security.MessageDigest.getInstance("SHA-256").digest("access-token".toByteArray())
        assertEquals(
            Base64.getUrlEncoder().withoutPadding().encodeToString(expected),
            claims.text("ath"),
        )
        assertEquals(64, Base64.getUrlDecoder().decode(fields[2]).size)
        val unbound = proof.sign("POST", "https://host.example/v1/token", null).split('.')
        val otherClaims =
            WireJson.parseToJsonElement(String(Base64.getUrlDecoder().decode(unbound[1])))
        assertNull(otherClaims.text("ath"))
        assertNotEquals(claims.text("jti"), otherClaims.text("jti"))
    }

    @Test
    fun packagedJniValidatesInvitationsAndRetiresFailedHandshakes() {
        val id = "0".repeat(32)
        val secret = Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(32))
        val invitation =
            obj("v" to 1.json(), "id" to id.json(), "secret" to secret.json()).toString()
        val fragment =
            Base64.getUrlEncoder().withoutPadding().encodeToString(invitation.toByteArray())
        val link = "https://host.example/#pair=$fragment"
        assertTrue(Endpoint.isNativeInvitation(link))
        val start = NativePairing.request(obj("op" to "start".json(), "link" to link.json()))
        val handle = requireNotNull(start.long("handle"))
        try {
            assertEquals("wss://host.example/api/paired-rpc/$id", start.text("url"))
            assertFalse(start.toString().contains(secret))
            assertFalse(start.flag("ready"))
            val invalid = runCatching {
                NativePairing.request(
                    obj(
                        "op" to "finish".json(),
                        "handle" to handle.json(),
                        "record" to "AA==".json(),
                    )
                )
            }
            assertTrue(invalid.exceptionOrNull() is ClientException)
            assertTrue(
                runCatching {
                        NativePairing.request(obj("op" to "poll".json(), "handle" to handle.json()))
                    }
                    .isFailure
            )
        } finally {
            NativePairing.request(obj("op" to "close".json(), "handle" to handle.json()))
        }
    }
}
