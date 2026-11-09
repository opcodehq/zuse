package sh.zuse.core

/** Retry a dropped healthy link immediately; flapping links retain bounded jittered backoff. */
class RetryBackoff {
    var attempt = 0
        private set

    fun nextDelay(
        healthyForMillis: Long = 0,
        jitter: Double = kotlin.random.Random.nextDouble(0.75, 1.25),
    ): Long {
        require(jitter in 0.75..1.25)
        if (healthyForMillis >= 30_000) attempt = 0
        val delay =
            if (attempt == 0) 0L
            else ((1000L shl (attempt - 1).coerceAtMost(5)) * jitter).toLong().coerceAtMost(30_000)
        attempt = (attempt + 1).coerceAtMost(32)
        return delay
    }
}
