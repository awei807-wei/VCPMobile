package com.vcp.mobile.service

/** 纯 JVM 可测的恢复文件写入租约与有限重试状态。 */
internal class SseDumpRetryLedger(private val maxAttempts: Int = DEFAULT_MAX_ATTEMPTS) {
    enum class BeginResult {
        STARTED,
        PENDING,
        PUBLISHED,
        EXHAUSTED
    }

    enum class FinishResult {
        PUBLISHED,
        RETRY_SCHEDULED,
        EXHAUSTED,
        IGNORED
    }

    companion object {
        const val DEFAULT_MAX_ATTEMPTS = 3
    }

    private var attempts = 0
    private var pending = false
    private var attemptInFlight = false
    private var retryWaiting = false
    private var published = false
    private var exhausted = false

    @Synchronized
    fun beginAttempt(): BeginResult {
        if (published) return BeginResult.PUBLISHED
        if (exhausted) return BeginResult.EXHAUSTED
        if (attemptInFlight || retryWaiting) return BeginResult.PENDING
        if (attempts >= maxAttempts.coerceAtLeast(1)) {
            exhausted = true
            pending = false
            return BeginResult.EXHAUSTED
        }
        attempts += 1
        attemptInFlight = true
        pending = true
        return BeginResult.STARTED
    }

    @Synchronized
    fun finishAttempt(success: Boolean): FinishResult {
        if (!attemptInFlight) return FinishResult.IGNORED
        attemptInFlight = false
        if (success) {
            published = true
            pending = false
            retryWaiting = false
            return FinishResult.PUBLISHED
        }
        if (attempts >= maxAttempts.coerceAtLeast(1)) {
            exhausted = true
            pending = false
            retryWaiting = false
            return FinishResult.EXHAUSTED
        }
        retryWaiting = true
        pending = true
        return FinishResult.RETRY_SCHEDULED
    }

    @Synchronized
    fun markRetryReady(): Boolean {
        if (!retryWaiting || published || exhausted) return false
        retryWaiting = false
        return true
    }

    @Synchronized
    fun hasPendingReference(): Boolean = pending

    @Synchronized
    fun attemptCount(): Int = attempts
}
