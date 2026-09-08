package com.vcp.mobile.service

import android.content.Context
import kotlinx.coroutines.CoroutineScope
import java.io.OutputStream

/** 管理 SSE 会话，并强制完整身份与 generation 所有权。 */
class SseSessionManager(
    context: Context,
    scope: CoroutineScope,
    onSessionsChanged: () -> Unit,
    appInForeground: () -> Boolean,
    eventSourceFactory: SseEventSourceFactory? = null,
) {
    private val core = SseSessionManagerCore(
        context = context,
        scope = scope,
        onSessionsChanged = onSessionsChanged,
        appInForeground = appInForeground,
        eventSourceFactory = eventSourceFactory,
    )

    fun hasRunningSessions(): Boolean = core.hasRunningSessions()

    fun runningSessionCount(): Int = core.runningSessionCount()

    fun start(command: SseProxyCommand, output: OutputStream): SseSocketLease {
        return core.start(command, output)
    }

    fun resume(
        key: StreamSessionKey,
        startIndex: Int,
        output: OutputStream,
        expectedGeneration: Long,
    ): SseSocketLease? {
        return core.resume(key, startIndex, output, expectedGeneration)
    }

    fun prepareResume(key: StreamSessionKey, expectedGeneration: Long): Boolean {
        return core.prepareResume(key, expectedGeneration)
    }

    fun prepareResume(
        key: StreamSessionKey,
        expectedGeneration: Long,
        output: OutputStream,
    ): Boolean {
        return core.prepareResume(key, expectedGeneration, output)
    }

    fun cancelResume(key: StreamSessionKey, expectedGeneration: Long): Boolean {
        return core.cancelResume(key, expectedGeneration)
    }

    fun query(key: StreamSessionKey, output: OutputStream) {
        core.query(key, output)
    }

    fun stop(key: StreamSessionKey, expectedGeneration: Long) {
        core.stop(key, expectedGeneration)
    }

    fun onSocketDisconnected(binding: SseSocketLease) {
        core.onSocketDisconnected(binding)
    }

    fun close() {
        core.close()
    }
}
