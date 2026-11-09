package sh.zuse.core

import kotlinx.serialization.json.JsonElement

/** A live socket is not proof that a host's old run status is still fresh. */
object CanonicalRuntime {
    fun status(
        row: JsonElement?,
        chatId: String,
        nowMillis: Long = System.currentTimeMillis(),
    ): String? {
        if (row?.text("chatId") != chatId) return null
        val updated =
            runCatching { java.time.Instant.parse(row.text("updatedAt")).toEpochMilli() }
                .getOrNull() ?: return null
        val age = nowMillis - updated
        if (age !in -5_000L..45_000L) return null
        return row.text("status")?.takeIf {
            it in listOf("idle", "working", "awaitingInput", "errored")
        }
    }
}
