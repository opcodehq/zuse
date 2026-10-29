package sh.zuse.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import sh.zuse.core.*

/** Atomic encrypted files in noBackupFilesDir, bound to a non-exportable Android Keystore key. */
class Vault(context: Context) {
    private val directory = File(context.noBackupFilesDir, "native-v1").apply { mkdirs() }
    private val lock = Mutex()
    private val key: SecretKey by lazy {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey("zuse.native.storage", null) as? SecretKey)
            ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
                .apply {
                    init(
                        KeyGenParameterSpec.Builder(
                                "zuse.native.storage",
                                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                            )
                            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                            .setKeySize(256)
                            .build()
                    )
                }
                .generateKey()
    }

    private fun file(identity: String) =
        AtomicFile(
            File(
                directory,
                MessageDigest.getInstance("SHA-256").digest(identity.toByteArray()).joinToString(
                    ""
                ) {
                    "%02x".format(it)
                },
            )
        )

    private fun readUnsafe(identity: String): String? {
        val file = file(identity)
        if (!file.baseFile.exists()) return null
        val bytes = file.readFully()
        require(bytes.size >= 28) { "The encrypted record is damaged." }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
        cipher.updateAAD(identity.toByteArray())
        return cipher.doFinal(bytes.copyOfRange(12, bytes.size)).toString(Charsets.UTF_8)
    }

    private fun writeUnsafe(identity: String, value: String?) {
        val file = file(identity)
        if (value == null) {
            file.delete()
            return
        }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key)
        cipher.updateAAD(identity.toByteArray())
        val bytes = cipher.iv + cipher.doFinal(value.toByteArray())
        val stream = file.startWrite()
        try {
            stream.write(bytes)
            file.finishWrite(stream)
        } catch (error: Throwable) {
            file.failWrite(stream)
            throw error
        }
    }

    // Acquire on the caller before dispatching IO so successive UI actions retain their order.
    private suspend fun <T> storage(block: () -> T): T =
        lock.withLock { withContext(Dispatchers.IO) { block() } }

    suspend fun read(identity: String): String? = storage { readUnsafe(identity) }

    suspend fun write(identity: String, value: String?) = storage { writeUnsafe(identity, value) }

    private fun prepareIntentUnsafe(
        identity: String,
        body: kotlinx.serialization.json.JsonObject,
        prompt: String? = null,
    ): kotlinx.serialization.json.JsonElement {
        val stored = readUnsafe(identity)?.let(WireJson::parseToJsonElement)
        require(stored == null || stored.field("body") == body) {
            "A previous request has an uncertain outcome. Refresh and retry the same selections first."
        }
        return stored
            ?: obj(
                    "id" to java.util.UUID.randomUUID().toString().json(),
                    "body" to body,
                    "prompt" to prompt?.json(),
                )
                .also { writeUnsafe(identity, it.toString()) }
    }

    suspend fun prepareIntent(
        identity: String,
        body: kotlinx.serialization.json.JsonObject,
        idField: String = "idempotencyKey",
    ): kotlinx.serialization.json.JsonObject = storage {
        val record = prepareIntentUnsafe(identity, body)
        kotlinx.serialization.json.JsonObject(
            body + (idField to requireNotNull(record.text("id")).json())
        )
    }

    /** The first prompt is atomically bound to the operation, without locking later text edits. */
    suspend fun prepareChatIntent(
        identity: String,
        body: kotlinx.serialization.json.JsonObject,
        prompt: String,
    ): Pair<kotlinx.serialization.json.JsonObject, String> = storage {
        val record = prepareIntentUnsafe(identity, body, prompt)
        kotlinx.serialization.json.JsonObject(
            body + ("operationId" to requireNotNull(record.text("id")).json())
        ) to (record.text("prompt") ?: prompt)
    }

    private fun metadataUnsafe(identity: String): SessionMetadata {
        val encoded = readUnsafe("archive:$identity") ?: return SessionMetadata()
        val decoded = decodeSessionMetadata(encoded)
        if (decoded.legacyMessages != null) {
            val existing = readUnsafe("history:$identity")
            // Validate before removing the old copy; unreadable data is never replaced implicitly.
            if (existing != null) WireJson.decodeFromString<List<Message>>(existing)
            else writeUnsafe("history:$identity", WireJson.encodeToString(decoded.legacyMessages))
            writeUnsafe("archive:$identity", WireJson.encodeToString(decoded.metadata))
        }
        return decoded.metadata
    }

    private fun historyUnsafe(identity: String): List<Message> =
        readUnsafe("history:$identity")?.let { WireJson.decodeFromString<List<Message>>(it) }
            ?: emptyList()

    suspend fun readMetadata(identity: String): SessionMetadata = storage {
        metadataUnsafe(identity)
    }

    suspend fun readHistory(identity: String): List<Message> = storage { historyUnsafe(identity) }

    suspend fun archive(identity: String): SessionArchive = storage {
        SessionArchive(metadataUnsafe(identity), historyUnsafe(identity))
    }

    /** Draft/outbox writes never read or serialize the potentially large history file. */
    suspend fun updateMetadata(
        identity: String,
        update: (SessionMetadata) -> SessionMetadata,
    ): SessionMetadata = storage {
        update(metadataUnsafe(identity)).also {
            writeUnsafe("archive:$identity", WireJson.encodeToString(it))
        }
    }

    suspend fun saveHistory(identity: String, messages: List<Message>) = storage {
        writeUnsafe("history:$identity", WireJson.encodeToString(messages))
    }
}
