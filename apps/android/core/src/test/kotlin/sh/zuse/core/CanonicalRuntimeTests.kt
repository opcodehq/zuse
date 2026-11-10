package sh.zuse.core

import kotlin.test.*

class CanonicalRuntimeTests {
    @Test
    fun `only fresh status for the selected chat can show a running turn`() {
        val now = 1_700_000_000_000L
        val row =
            obj(
                "chatId" to "chat".json(),
                "status" to "working".json(),
                "updatedAt" to java.time.Instant.ofEpochMilli(now).toString().json(),
            )
        assertEquals("working", CanonicalRuntime.status(row, "chat", now))
        assertNull(CanonicalRuntime.status(row, "other", now))
        assertNull(CanonicalRuntime.status(row, "chat", now + 46_000))
        assertNull(CanonicalRuntime.status(row, "chat", now - 6_000))
        assertNull(CanonicalRuntime.status(obj(), "chat", now))
    }
}
