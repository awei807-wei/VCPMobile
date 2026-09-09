package com.vcp.mobile.service

import okhttp3.Request
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener

/** 可替换的 SSE 工厂，便于在安装门闩测试中控制返回时序。 */
fun interface SseEventSourceFactory {
    fun create(request: Request, listener: EventSourceListener): EventSource
}
