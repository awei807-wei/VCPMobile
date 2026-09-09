package com.vcp.mobile.service

/** 物理锁释放失败时的短时、有界重试调度器。 */
internal class ForegroundLockReleaseScheduler(
    private val postDelayed: (Runnable, Long) -> Boolean,
    private val removeCallbacks: (Runnable) -> Unit,
    private val releaseLocks: () -> Boolean,
    private val logError: (String) -> Unit,
) {
    companion object {
        val RETRY_DELAYS_MS = longArrayOf(100L, 250L, 500L, 1_000L, 2_000L)
    }

    private var retryRunnable: Runnable? = null
    private var retryIndex = 0
    private var generation = 0L

    @Synchronized
    fun cancel() {
        generation += 1
        retryRunnable?.let(removeCallbacks)
        retryRunnable = null
        retryIndex = 0
    }

    @Synchronized
    fun release() {
        cancel()
        attemptRelease(generation)
    }

    @Synchronized
    private fun attemptRelease(expectedGeneration: Long) {
        if (expectedGeneration != generation) {
            return
        }
        if (!releaseLocks()) {
            retryIndex = 0
            return
        }
        if (retryIndex >= RETRY_DELAYS_MS.size) {
            retryRunnable = null
            logError("物理保活锁在有界重试后仍未释放")
            return
        }
        val delay = RETRY_DELAYS_MS[retryIndex]
        retryIndex += 1
        val callbackGeneration = generation
        val runnable = Runnable {
            synchronized(this) {
                if (callbackGeneration != generation) {
                    return@synchronized
                }
                retryRunnable = null
                attemptRelease(callbackGeneration)
            }
        }
        retryRunnable = runnable
        var schedulingFailureLogged = false
        val scheduled = try {
            postDelayed(runnable, delay)
        } catch (error: Throwable) {
            schedulingFailureLogged = true
            logError(
                "调度物理保活锁释放重试（${delay}ms）抛错：${error.message}，" +
                    "改为立即执行有界重试"
            )
            false
        }
        if (!scheduled) {
            retryRunnable = null
            if (!schedulingFailureLogged) {
                logError("无法调度物理保活锁释放重试（${delay}ms），改为立即执行有界重试")
            }
            attemptRelease(callbackGeneration)
        }
    }
}
