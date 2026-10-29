package sh.zuse.core

import kotlinx.serialization.json.*

/** Byte arrays preserve UTF-8 across JNI, including supplementary Unicode in RPC payloads. */
object NativePairing {
    private val loaded: Boolean by lazy {
        System.loadLibrary("zuse_pairing")
        true
    }

    @JvmStatic private external fun exchange(input: ByteArray): ByteArray?

    fun request(request: JsonObject): JsonObject {
        check(loaded)
        val response =
            exchange(request.toString().toByteArray(Charsets.UTF_8))
                ?: throw ClientException("Native pairing is unavailable.")
        val result = WireJson.parseToJsonElement(response.toString(Charsets.UTF_8)).jsonObject
        if (result.text("error") != null)
            throw ClientException(
                "Paired connection unavailable. Reconnect with a valid invitation."
            )
        return result
    }
}
