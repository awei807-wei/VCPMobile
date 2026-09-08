package com.vcp.mobile.service

/**
 * 在 registry 当前 generation 仍有效时安装 EventSource。
 *
 * stop/replacement 可能发生在工厂返回之后。先锁 registry，再锁 session，
 * 才能保证旧 session 不会在取消后又留下一个未跟踪的 HTTP 流。
 */
class SseEventSourceInstallGate<T : Any, S : Any>(
    private val registry: StreamSessionRegistry<T>,
    private val sessionOf: (StreamSessionRegistry.Lease<T>) -> T,
    private val attach: (T, S) -> Unit,
    private val cancel: (S) -> Unit
) {
    fun attachIfCurrent(lease: StreamSessionRegistry.Lease<T>, source: S): Boolean {
        val session = sessionOf(lease)
        val attached = registry.withCurrent(lease) {
            synchronized(session) {
                attach(session, source)
                true
            }
        } == true
        if (!attached) cancel(source)
        return attached
    }
}
