package com.vcp.mobile.service

import android.util.Log

/** helper 会话的取消与输出关闭动作，集中保证异常可观测且可重入。 */
internal object SseSessionLifecycle {
    private const val TAG = "VcpSseSessions"

    fun cancel(session: SseStreamSession) {
        synchronized(session) {
            try {
                session.eventSource?.cancel()
            } catch (error: Exception) {
                Log.w(TAG, "取消 EventSource 失败：result=error")
            }
            session.eventSource = null
            closeOutput(session)
        }
    }

    fun closeOutput(session: SseStreamSession) {
        val output = synchronized(session) {
            session.activeSocketOutputStream.also {
                session.activeSocketOutputStream = null
            }
        }
        output?.let(::closeOutput)
    }

    /**
     * 关闭一个已经从 session ownership 中移出的输出。
     *
     * resume 的 handoff 需要先把新输出完成握手和回放，再提交为 active
     * output；旧输出因此不能再通过 [closeOutput] 从 session 中查找，而必须
     * 由调用方持有并显式关闭。
     */
    fun closeOutput(output: java.io.OutputStream) {
        try {
            output.close()
        } catch (error: Exception) {
            Log.w(TAG, "关闭流输出失败：result=error")
        }
    }
}
