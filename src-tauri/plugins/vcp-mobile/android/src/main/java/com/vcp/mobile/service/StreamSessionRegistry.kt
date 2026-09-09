package com.vcp.mobile.service

import java.util.concurrent.atomic.AtomicLong

/** helper 流的线程安全所有权 registry。 */
class StreamSessionRegistry<T : Any> {
    data class Lease<T : Any>(
        val key: StreamSessionKey,
        val generation: Long,
        val value: T
    )

    private val lock = Any()
    private val current = HashMap<StreamSessionKey, Lease<T>>()
    private val nextGeneration = AtomicLong(0L)

    /** 安装新 generation，不执行替换副作用。 */
    fun install(key: StreamSessionKey, value: T): Lease<T> = install(key, value) {}

    /** 原子替换 generation，然后取消旧值；旧值不得重新写回 registry。 */
    fun install(
        key: StreamSessionKey,
        value: T,
        onReplaced: (Lease<T>) -> Unit
    ): Lease<T> {
        val result = synchronized(lock) {
            val generation = nextGeneration.incrementAndGet()
            val old = current.put(key, Lease(key, generation, value))
            current.getValue(key) to old
        }
        result.second?.let(onReplaced)
        return result.first
    }

    fun current(key: StreamSessionKey): Lease<T>? = synchronized(lock) { current[key] }

    fun isCurrent(lease: Lease<T>): Boolean = synchronized(lock) {
        val active = current[lease.key]
        active?.generation == lease.generation && active.value === lease.value
    }

    fun isCurrent(key: StreamSessionKey, generation: Long): Boolean = synchronized(lock) {
        current[key]?.generation == generation
    }

    /** 在 registry 锁内执行当前 lease 的受控发布，阻止 replacement 穿过发布窗口。 */
    fun <R> withCurrent(lease: Lease<T>, action: (T) -> R): R? = synchronized(lock) {
        val active = current[lease.key]
        if (active?.generation == lease.generation && active.value === lease.value) {
            action(lease.value)
        } else {
            null
        }
    }

    /** 只移除当前 [lease]，旧 callback 不能移除替换后的流。 */
    fun removeIfCurrent(lease: Lease<T>): Boolean = synchronized(lock) {
        val active = current[lease.key]
        if (active?.generation == lease.generation && active.value === lease.value) {
            current.remove(lease.key)
            true
        } else {
            false
        }
    }

    /** 移除当前 generation；传入 expectedGeneration 时必须精确匹配。 */
    fun removeCurrent(key: StreamSessionKey, expectedGeneration: Long? = null): Lease<T>? {
        return synchronized(lock) {
            val active = current[key] ?: return@synchronized null
            if (expectedGeneration != null && active.generation != expectedGeneration) {
                return@synchronized null
            }
            current.remove(key)
        }
    }

    fun snapshot(): List<Lease<T>> = synchronized(lock) { current.values.toList() }

    /** 服务销毁时移除所有值，但 generation 计数保持单调。 */
    fun clear(): List<Lease<T>> = synchronized(lock) {
        val values = current.values.toList()
        current.clear()
        values
    }

    val size: Int
        get() = synchronized(lock) { current.size }
}
