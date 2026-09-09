package com.vcp.mobile.service

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.io.OutputStream
import java.util.concurrent.atomic.AtomicLong

/** 负责同一 helper generation 的 socket 接管、恢复租约和超时清理。 */
internal class SseSessionHandoffCoordinator(
    private val registry: StreamSessionRegistry<SseStreamSession>,
    private val scope: CoroutineScope,
    private val onSessionsChanged: () -> Unit,
) {
    companion object {
        private const val TAG = "VcpSseSessions"
        // A prepare command is a bounded takeover lease. It is long enough for
        // the Rust side's helper handshake plus generation binding, while still
        // ensuring a crashed caller cannot mask a real disconnect forever.
        private const val TAKEOVER_TIMEOUT_MS = 30_000L
    }

    internal enum class DisconnectResult {
        STALE,
        RETAINED,
        REMOVED,
    }

    internal sealed class AttachAndReplayResult {
        data class Attached(val connectionGeneration: Long) : AttachAndReplayResult()

        data object Failed : AttachAndReplayResult()
    }

    private data class DisconnectDecision(
        val result: DisconnectResult,
        val detachedOutput: OutputStream?,
    )

    private val nextTakeoverToken = AtomicLong(0L)

    fun prepareResume(key: StreamSessionKey, expectedGeneration: Long): Boolean {
        val lease = registry.current(key) ?: return false
        if (lease.generation != expectedGeneration) return false
        val session = lease.value
        val prepared = registry.withCurrent(lease) {
            synchronized(session) {
                beginTakeoverLocked(lease, session)
            }
        }
        return prepared == true
    }

    fun prepareResume(
        key: StreamSessionKey,
        expectedGeneration: Long,
        output: OutputStream,
    ): Boolean {
        val lease = registry.current(key)
        if (lease == null || lease.generation != expectedGeneration) {
            SseSessionWire.writeMissingSession(output, key)
            return false
        }
        val prepared = registry.withCurrent(lease) {
            val session = lease.value
            synchronized(session) {
                writeTakeoverAck(lease, session, output)
            }
        } == true
        if (!prepared) {
            SseSessionWire.writeMissingSession(output, key)
        }
        return prepared
    }

    fun cancelResume(key: StreamSessionKey, expectedGeneration: Long): Boolean {
        val lease = registry.current(key) ?: return false
        if (lease.generation != expectedGeneration) return false
        var removed = false
        val cancelled = registry.withCurrent(lease) {
            synchronized(lease.value) {
                val session = lease.value
                val hadPending = session.pendingTakeoverToken != null
                val oldConnectionLost = hasLostPendingConnection(session)
                clearTakeoverLocked(session)
                if (oldConnectionLost) {
                    registry.removeIfCurrent(lease)
                    SseSessionLifecycle.cancel(session)
                    removed = true
                }
                hadPending
            }
        } == true
        if (removed) onSessionsChanged()
        return cancelled
    }

    fun attachAndReplay(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        session: SseStreamSession,
        startIndex: Int,
        output: OutputStream,
    ): AttachAndReplayResult? {
        return registry.withCurrent(lease) {
            synchronized(session) {
                val previousOutput = session.activeSocketOutputStream
                val connectionGeneration = session.socketGeneration + 1
                val boundedStart = startIndex.coerceAtMost(session.eventBuffer.size)
                beginTakeoverLocked(lease, session)
                try {
                    SseSessionWire.writeGenerationAck(output, session, lease.generation)
                    replayEvents(session, boundedStart, output)
                    commitAttach(session, output, previousOutput, connectionGeneration)
                    AttachAndReplayResult.Attached(connectionGeneration)
                } catch (error: Exception) {
                    Log.e(TAG, "回放流事件失败：result=error")
                    rollbackAttach(lease, session, output, previousOutput)
                    AttachAndReplayResult.Failed
                }
            }
        }
    }

    fun disconnectSocket(
        binding: SseSocketLease,
        retainForRecovery: Boolean = true,
    ): DisconnectResult {
        val lease = binding.sessionLease
        val session = lease.value
        val decision = registry.withCurrent(lease) {
            synchronized(session) {
                disconnectLocked(binding, retainForRecovery)
            }
        } ?: DisconnectDecision(DisconnectResult.STALE, null)
        decision.detachedOutput?.let(SseSessionLifecycle::closeOutput)
        return decision.result
    }

    private fun writeTakeoverAck(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        session: SseStreamSession,
        output: OutputStream,
    ): Boolean {
        val alreadyPending = session.pendingTakeoverToken != null
        beginTakeoverLocked(lease, session)
        return try {
            SseSessionWire.writeTakeoverAck(output, session, lease.generation)
            true
        } catch (error: Exception) {
            if (!alreadyPending) clearTakeoverLocked(session)
            Log.e(TAG, "写入 helper 接管准备 ACK 失败：result=error")
            false
        }
    }

    private fun replayEvents(
        session: SseStreamSession,
        boundedStart: Int,
        output: OutputStream,
    ) {
        for (index in boundedStart until session.eventBuffer.size) {
            SseSocketCodec.write(output, session.eventBuffer[index].toString())
        }
    }

    private fun commitAttach(
        session: SseStreamSession,
        output: OutputStream,
        previousOutput: OutputStream?,
        connectionGeneration: Long,
    ) {
        // Registry/session locks make this assignment atomic with callbacks and replacement.
        session.socketGeneration = connectionGeneration
        session.activeSocketOutputStream = output
        clearTakeoverLocked(session)
        if (previousOutput != null && previousOutput !== output) {
            SseSessionLifecycle.closeOutput(previousOutput)
        }
    }

    private fun rollbackAttach(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        session: SseStreamSession,
        output: OutputStream,
        previousOutput: OutputStream?,
    ) {
        // The candidate output was never installed; the previous socket remains the owner.
        if (output !== previousOutput) SseSessionLifecycle.closeOutput(output)
        val oldConnectionLost = hasLostPendingConnection(session)
        clearTakeoverLocked(session)
        if (oldConnectionLost) {
            beginTakeoverLocked(lease, session)
            session.pendingTakeoverConnectionLost = true
        }
    }

    private fun disconnectLocked(
        binding: SseSocketLease,
        retainForRecovery: Boolean,
    ): DisconnectDecision {
        val session = binding.sessionLease.value
        if (session.socketGeneration != binding.connectionGeneration) {
            return DisconnectDecision(DisconnectResult.STALE, null)
        }
        if (!retainForRecovery) {
            registry.removeIfCurrent(binding.sessionLease)
            SseSessionLifecycle.cancel(session)
            return DisconnectDecision(DisconnectResult.REMOVED, null)
        }
        if (session.pendingTakeoverConnectionGeneration == binding.connectionGeneration) {
            session.pendingTakeoverConnectionLost = true
            return DisconnectDecision(DisconnectResult.RETAINED, null)
        }
        if (session.isCompleted) {
            return detachCompletedSession(session)
        }
        beginTakeoverLocked(binding.sessionLease, session)
        session.pendingTakeoverConnectionLost = true
        return detachActiveSocket(session)
    }

    private fun detachCompletedSession(session: SseStreamSession): DisconnectDecision {
        val output = session.activeSocketOutputStream
        session.activeSocketOutputStream = null
        return DisconnectDecision(DisconnectResult.RETAINED, output)
    }

    private fun detachActiveSocket(session: SseStreamSession): DisconnectDecision {
        val output = session.activeSocketOutputStream
        session.activeSocketOutputStream = null
        return DisconnectDecision(DisconnectResult.RETAINED, output)
    }

    private fun hasLostPendingConnection(session: SseStreamSession): Boolean {
        return session.pendingTakeoverToken != null &&
            session.pendingTakeoverConnectionLost &&
            session.pendingTakeoverConnectionGeneration == session.socketGeneration
    }

    private fun beginTakeoverLocked(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        session: SseStreamSession,
    ): Boolean {
        if (session.pendingTakeoverToken != null) return true
        val token = nextTakeoverToken.incrementAndGet()
        session.pendingTakeoverToken = token
        session.pendingTakeoverConnectionGeneration = session.socketGeneration
        scope.launch {
            delay(TAKEOVER_TIMEOUT_MS)
            expireTakeover(lease, token)
        }
        return true
    }

    private fun clearTakeoverLocked(session: SseStreamSession): Boolean {
        val hadPending = session.pendingTakeoverToken != null
        session.pendingTakeoverToken = null
        session.pendingTakeoverConnectionGeneration = null
        session.pendingTakeoverConnectionLost = false
        return hadPending
    }

    private fun expireTakeover(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        token: Long,
    ) {
        val expired = registry.withCurrent(lease) {
            val session = lease.value
            synchronized(session) {
                if (session.pendingTakeoverToken != token) {
                    false
                } else {
                    clearTakeoverLocked(session)
                    registry.removeIfCurrent(lease)
                    SseSessionLifecycle.cancel(session)
                    true
                }
            }
        } == true
        if (expired) onSessionsChanged()
    }
}
