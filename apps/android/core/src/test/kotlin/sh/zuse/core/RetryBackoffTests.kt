package sh.zuse.core

import kotlin.test.*

class RetryBackoffTests {
    @Test
    fun `fast first recovery does not turn flapping into immediate reconnect loops`() {
        val backoff = RetryBackoff()
        assertEquals(0L, backoff.nextDelay(jitter = 1.0))
        assertEquals(1000L, backoff.nextDelay(healthyForMillis = 100, jitter = 1.0))
        assertEquals(2000L, backoff.nextDelay(healthyForMillis = 100, jitter = 1.0))
        repeat(40) { assertTrue(backoff.nextDelay(jitter = 1.25) <= 30_000) }
        assertEquals(0L, backoff.nextDelay(healthyForMillis = 30_000, jitter = 1.0))
        assertEquals(750L, backoff.nextDelay(jitter = 0.75))
    }
}
