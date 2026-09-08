package com.vcp.mobile.service

import android.util.Log
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import org.json.JSONObject

/** 负责 EventSource 回调、事件缓冲和会话完成状态。 */
internal class SseSessionEventHandler(
    private val registry: StreamSessionRegistry<SseStreamSession>,
    private val notifier: SseStreamNotifier,
    private val dumpCoordinator: SseSessionDumpCoordinator,
    private val onSessionsChanged: () -> Unit,
) {
    companion object {
        private const val TAG = "VcpSseSessions"
    }

    fun createListener(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
    ): EventSourceListener {
        return object : EventSourceListener() {
            override fun onOpen(eventSource: EventSource, response: Response) {
                sendEvent(lease, "open", "")
            }

            override fun onEvent(
                eventSource: EventSource,
                id: String?,
                type: String?,
                data: String,
            ) {
                sendEvent(lease, "message", data)
            }

            override fun onFailure(
                eventSource: EventSource,
                t: Throwable?,
                response: Response?,
            ) {
                finishWithFailure(
                    lease,
                    t?.message ?: response?.message ?: "未知网络错误",
                    response?.code ?: 0,
                )
            }

            override fun onClosed(eventSource: EventSource) {
                finishSuccessfully(lease)
            }
        }
    }

    private fun finishWithFailure(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        errorMessage: String,
        statusCode: Int,
    ) {
        if (!markCompleted(lease, "error")) return
        val error = JSONObject().apply {
            put("error", errorMessage)
            put("status", statusCode)
        }
        sendEvent(lease, "error", error.toString())
        if (registry.isCurrent(lease)) {
            notifier.show(lease.value, isSuccess = false, errorMsg = errorMessage)
        }
        dumpCoordinator.cleanup(lease)
        onSessionsChanged()
    }

    private fun finishSuccessfully(lease: StreamSessionRegistry.Lease<SseStreamSession>) {
        if (!markCompleted(lease, "completed")) return
        sendEvent(lease, "closed", "")
        if (registry.isCurrent(lease)) {
            notifier.show(lease.value, isSuccess = true, errorMsg = null)
        }
        dumpCoordinator.cleanup(lease)
        onSessionsChanged()
    }

    private fun markCompleted(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        reason: String,
    ): Boolean {
        return registry.withCurrent(lease) {
            synchronized(lease.value) {
                if (lease.value.isCompleted) {
                    false
                } else {
                    lease.value.isCompleted = true
                    lease.value.lastFinishReason = reason
                    true
                }
            }
        } == true
    }

    private fun sendEvent(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        eventType: String,
        data: String,
    ) {
        val session = lease.value
        registry.withCurrent(lease) {
            synchronized(session) {
                val event = buildEvent(lease, session, eventType, data)
                session.eventBuffer.add(event)
                session.activeSocketOutputStream?.let { output ->
                    try {
                        SseSocketCodec.write(output, event.toString().orEmpty())
                    } catch (error: Exception) {
                        Log.w(TAG, "写入流事件失败：result=error")
                        SseSessionLifecycle.closeOutput(session)
                    }
                }
            }
        }
    }

    private fun buildEvent(
        lease: StreamSessionRegistry.Lease<SseStreamSession>,
        session: SseStreamSession,
        eventType: String,
        data: String,
    ): JSONObject {
        return JSONObject().apply {
            put("requestId", session.requestId)
            put("messageId", session.requestId)
            put("ownerType", session.key.ownerType)
            put("ownerId", session.key.ownerId)
            put("topicId", session.key.topicId)
            put("generation", lease.generation)
            put("eventType", eventType)
            put("eventData", data)
            put("index", session.eventBuffer.size)
        }
    }
}
