package com.vcp.mobile.service

import android.content.Context
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import okhttp3.OkHttpClient
import okhttp3.sse.EventSource
import okhttp3.sse.EventSources
import org.json.JSONObject
import java.io.OutputStream
import java.util.concurrent.TimeUnit

/** SseSessionManager 的会话命令编排与 authoritative registry。 */
internal class SseSessionManagerCore(
    context: Context,
    private val scope: CoroutineScope,
    private val onSessionsChanged: () -> Unit,
    appInForeground: () -> Boolean,
    eventSourceFactory: SseEventSourceFactory? = null,
) {
    companion object {
        private const val TAG = "VcpSseSessions"
    }

    private val appContext = context.applicationContext
    private val registry = StreamSessionRegistry<SseStreamSession>()
    private val startEpochLock = Any()
    private val latestStartEpochs = HashMap<StreamSessionKey, Long>()
    private val notifier = SseStreamNotifier(appContext, appInForeground)
    private val httpClient = OkHttpClient.Builder()
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .connectTimeout(15, TimeUnit.SECONDS)
        .build()
    private val sourceFactory = eventSourceFactory ?: SseEventSourceFactory { request, listener ->
        EventSources.createFactory(httpClient).newEventSource(request, listener)
    }
    private val sourceInstallGate = SseEventSourceInstallGate<SseStreamSession, EventSource>(
        registry = registry,
        sessionOf = { lease -> lease.value },
        attach = { session, source -> session.eventSource = source },
        cancel = { source -> cancelSource(source) },
    )
    private val dumpCoordinator = SseSessionDumpCoordinator(
        context = appContext,
        scope = scope,
        registry = registry,
        onSessionsChanged = onSessionsChanged,
    )
    private val handoff = SseSessionHandoffCoordinator(
        registry = registry,
        scope = scope,
        onSessionsChanged = onSessionsChanged,
    )
    private val eventHandler = SseSessionEventHandler(
        registry = registry,
        notifier = notifier,
        dumpCoordinator = dumpCoordinator,
        onSessionsChanged = onSessionsChanged,
    )

    fun hasRunningSessions(): Boolean = registry.snapshot().any { !it.value.isCompleted }

    fun runningSessionCount(): Int = registry.snapshot().count {
        !it.value.isCompleted || it.value.dumpRetryLedger.hasPendingReference()
    }

    fun start(command: SseProxyCommand, output: OutputStream): SseSocketLease {
        val url = command.url?.takeIf(String::isNotBlank)
            ?: throw IllegalArgumentException("start 命令缺少 url")
        val requestEpoch = command.requestEpoch
            ?.takeIf { it > 0 }
            ?: throw IllegalArgumentException("start 命令缺少正整数 requestEpoch")
        val session = SseStreamSession(
            command.key,
            command.contextJson,
            requestEpoch = requestEpoch,
        ).apply {
            activeSocketOutputStream = output
            socketGeneration = 1L
        }
        val lease = installSession(command.key, requestEpoch, session)
        return try {
            SseSessionWire.writeGenerationAck(output, session, lease.generation)
            val listener = eventHandler.createListener(lease)
            val source = sourceFactory.create(SseSessionRequest.build(command, url), listener)
            if (!sourceInstallGate.attachIfCurrent(lease, source)) {
                throw IllegalStateException("流 generation 已失效")
            }
            onSessionsChanged()
            SseSocketLease(lease, session.socketGeneration)
        } catch (error: Exception) {
            registry.removeIfCurrent(lease)
            SseSessionLifecycle.cancel(session)
            onSessionsChanged()
            Log.e(TAG, "启动流失败：result=error")
            throw error
        }
    }

    private fun installSession(
        key: StreamSessionKey,
        requestEpoch: Long,
        session: SseStreamSession,
    ): StreamSessionRegistry.Lease<SseStreamSession> {
        return synchronized(startEpochLock) {
            val latest = latestStartEpochs[key]
            if (latest != null && requestEpoch <= latest) {
                throw IllegalStateException(
                    "拒绝迟到 start：requestEpoch=$requestEpoch, latest=$latest",
                )
            }
            latestStartEpochs[key] = requestEpoch
            registry.install(key, session) { replaced ->
                Log.i(TAG, "替换流：generation=${replaced.generation}, result=replaced")
                SseSessionLifecycle.cancel(replaced.value)
            }
        }.also { lease -> session.generation = lease.generation }
    }

    fun resume(
        key: StreamSessionKey,
        startIndex: Int,
        output: OutputStream,
        expectedGeneration: Long,
    ): SseSocketLease? {
        val lease = registry.current(key)
        if (lease == null) {
            SseSessionWire.writeMissingSession(output, key)
            return null
        }
        if (lease.generation != expectedGeneration) {
            Log.d(TAG, "忽略过期 resume：expected=$expectedGeneration，current=${lease.generation}")
            SseSessionWire.writeMissingSession(output, key)
            return null
        }
        val session = lease.value
        return when (val attachment = handoff.attachAndReplay(lease, session, startIndex, output)) {
            null, SseSessionHandoffCoordinator.AttachAndReplayResult.Failed -> null
            is SseSessionHandoffCoordinator.AttachAndReplayResult.Attached -> {
                finishResume(lease, attachment.connectionGeneration)
            }
        }
    }

    private fun finishResume(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        connectionGeneration: Long,
    ): SseSocketLease? {
        if (!registry.isCurrent(lease)) return null
        val binding = SseSocketLease(lease, connectionGeneration)
        return try {
            onSessionsChanged()
            binding
        } catch (error: Throwable) {
            Log.e(TAG, "resume 后更新物理保活锁失败，按 socket 断开清理：result=error")
            handoff.disconnectSocket(binding, retainForRecovery = false)
            throw error
        }
    }

    fun prepareResume(key: StreamSessionKey, expectedGeneration: Long): Boolean {
        return handoff.prepareResume(key, expectedGeneration)
    }

    fun prepareResume(
        key: StreamSessionKey,
        expectedGeneration: Long,
        output: OutputStream,
    ): Boolean {
        return handoff.prepareResume(key, expectedGeneration, output)
    }

    fun cancelResume(key: StreamSessionKey, expectedGeneration: Long): Boolean {
        return handoff.cancelResume(key, expectedGeneration)
    }

    fun query(key: StreamSessionKey, output: OutputStream) {
        val snapshot = querySnapshot(key)
        SseSessionWire.writeQueryResponse(
            output,
            key,
            snapshot.status,
            snapshot.generation,
            snapshot.finishReason,
            snapshot.lastEventIndex,
            snapshot.content,
        )
    }

    private data class QuerySnapshot(
        val status: String,
        val generation: Long?,
        val finishReason: String,
        val lastEventIndex: Int,
        val content: String,
    )

    private fun querySnapshot(key: StreamSessionKey): QuerySnapshot {
        val lease = registry.current(key)
            ?: return QuerySnapshot("not_found", null, "", -1, "")
        val session = lease.value
        val snapshot = registry.withCurrent(lease) {
            synchronized(session) {
                QuerySnapshot(
                    status = if (session.isCompleted) "completed" else "streaming",
                    generation = lease.generation,
                    finishReason = session.lastFinishReason ?: "",
                    lastEventIndex = session.eventBuffer.size - 1,
                    content = SseSessionContent.fullText(session.eventBuffer),
                )
            }
        }
        return snapshot ?: QuerySnapshot("not_found", null, "", -1, "")
    }

    fun stop(key: StreamSessionKey, expectedGeneration: Long) {
        val lease = registry.removeCurrent(key, expectedGeneration)
        if (lease == null) {
            Log.d(TAG, "忽略过期或重复 stop：expected=$expectedGeneration")
            return
        }
        Log.i(TAG, "停止流：generation=${lease.generation}, result=stopped")
        SseSessionLifecycle.cancel(lease.value)
        onSessionsChanged()
    }

    fun onSocketDisconnected(binding: SseSocketLease) {
        when (handoff.disconnectSocket(binding)) {
            SseSessionHandoffCoordinator.DisconnectResult.STALE -> {
                Log.d(TAG, "忽略旧 socket 断开回调：result=stale")
            }

            SseSessionHandoffCoordinator.DisconnectResult.RETAINED -> {
                dumpCoordinator.cleanup(binding.sessionLease)
                onSessionsChanged()
            }

            SseSessionHandoffCoordinator.DisconnectResult.REMOVED -> onSessionsChanged()
        }
    }

    fun close() {
        registry.clear().forEach { lease -> SseSessionLifecycle.cancel(lease.value) }
    }

    private fun cancelSource(source: EventSource) {
        try {
            source.cancel()
        } catch (error: Exception) {
            Log.w(TAG, "取消过期 EventSource 失败：result=error")
        }
    }
}
