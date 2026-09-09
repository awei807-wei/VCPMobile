package com.vcp.mobile.service

import android.content.Context
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** 管理完成流的恢复文件发布、有限重试和最终清理。 */
internal class SseSessionDumpCoordinator(
    private val context: Context,
    private val scope: CoroutineScope,
    private val registry: StreamSessionRegistry<SseStreamSession>,
    private val onSessionsChanged: () -> Unit,
) {
    companion object {
        private const val TAG = "VcpSsePersistence"
        private const val RETRY_DELAY_MS = 1000L
        private const val CLEANUP_DELAY_MS = 5 * 60 * 1000L
    }

    fun cleanup(lease: StreamSessionRegistry.Lease<SseStreamSession>) {
        val session = lease.value
        val decision = registry.withCurrent(lease) {
            synchronized(session) {
                if (!session.isCompleted || session.activeSocketOutputStream != null) {
                    SseDumpRetryLedger.BeginResult.PENDING
                } else {
                    session.dumpRetryLedger.beginAttempt()
                }
            }
        } ?: return
        when (decision) {
            SseDumpRetryLedger.BeginResult.STARTED -> attemptDump(lease)
            SseDumpRetryLedger.BeginResult.PUBLISHED -> scheduleCleanup(lease)
            SseDumpRetryLedger.BeginResult.EXHAUSTED -> exhaust(lease)
            SseDumpRetryLedger.BeginResult.PENDING -> Unit
        }
    }

    private fun attemptDump(lease: StreamSessionRegistry.Lease<SseStreamSession>) {
        val dumped = registry.withCurrent(lease) {
            SseSessionPersistence.dump(context, lease.value)
        } ?: return
        val result = registry.withCurrent(lease) {
            lease.value.dumpRetryLedger.finishAttempt(dumped)
        } ?: return
        when (result) {
            SseDumpRetryLedger.FinishResult.PUBLISHED -> {
                lease.value.dumpedToDisk = true
                Log.i(TAG, "恢复文件已发布：result=published")
                onSessionsChanged()
                cleanup(lease)
            }

            SseDumpRetryLedger.FinishResult.RETRY_SCHEDULED -> {
                Log.e(
                    TAG,
                    "恢复文件写入失败：result=retry_scheduled，" +
                        "（第 ${lease.value.dumpRetryLedger.attemptCount()} 次）",
                )
                scheduleRetry(lease)
            }

            SseDumpRetryLedger.FinishResult.EXHAUSTED -> {
                Log.e(TAG, "恢复文件多次写入失败，result=exhausted")
                exhaust(lease)
            }

            SseDumpRetryLedger.FinishResult.IGNORED -> Unit
        }
    }

    private fun scheduleRetry(lease: StreamSessionRegistry.Lease<SseStreamSession>) {
        scope.launch(Dispatchers.IO) {
            delay(RETRY_DELAY_MS)
            val ready = registry.withCurrent(lease) {
                lease.value.dumpRetryLedger.markRetryReady()
            } == true
            if (ready) cleanup(lease)
        }
    }

    private fun exhaust(lease: StreamSessionRegistry.Lease<SseStreamSession>) {
        if (!registry.removeIfCurrent(lease)) return
        SseSessionLifecycle.cancel(lease.value)
        onSessionsChanged()
    }

    private fun scheduleCleanup(lease: StreamSessionRegistry.Lease<SseStreamSession>) {
        val scheduled = registry.withCurrent(lease) {
            synchronized(lease.value) {
                if (lease.value.cleanupScheduled) {
                    false
                } else {
                    lease.value.cleanupScheduled = true
                    true
                }
            }
        } == true
        if (!scheduled) return
        scope.launch(Dispatchers.IO) {
            delay(CLEANUP_DELAY_MS)
            if (registry.removeIfCurrent(lease)) {
                Log.i(TAG, "移除已完成且断开的流：result=removed")
                onSessionsChanged()
            }
        }
    }
}
