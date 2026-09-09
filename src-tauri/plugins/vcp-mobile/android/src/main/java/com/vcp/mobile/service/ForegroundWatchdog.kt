package com.vcp.mobile.service

import android.content.Context
import android.os.Handler

/** 按 tag 与 generation 精确跟踪前台 lease 的超时回调。 */
internal class ForegroundWatchdog(
    private val handler: Handler,
    private val onTimeout: (Context, String, Long?) -> Unit
) {
    private val callbacks = ForegroundWatchdogLedger<Runnable>()

    fun schedule(context: Context, tag: String, generation: Long?, timeoutMs: Long) {
        if (timeoutMs <= 0) {
            return
        }
        cancel(tag, generation)
        val key = ForegroundWatchdogKey(tag, generation)
        lateinit var callback: Runnable
        callback = Runnable {
            callbacks.removeIfSame(key, callback)
            onTimeout(context, tag, generation)
        }
        callbacks.replace(key, callback)
        handler.postDelayed(callback, timeoutMs)
    }

    fun cancel(tag: String, generation: Long?) {
        callbacks.remove(ForegroundWatchdogKey(tag, generation))?.let(handler::removeCallbacks)
    }

    fun clear() {
        callbacks.values().forEach(handler::removeCallbacks)
        callbacks.clear()
    }
}
