package com.vcp.mobile.service

/** 纯 JVM 可测的 watchdog 账本，键包含完整 tag 与 generation。 */
internal data class ForegroundWatchdogKey(val tag: String, val generation: Long?)

internal class ForegroundWatchdogLedger<T> {
    private val values = HashMap<ForegroundWatchdogKey, T>()

    @Synchronized
    fun replace(key: ForegroundWatchdogKey, value: T): T? = values.put(key, value)

    @Synchronized
    fun remove(key: ForegroundWatchdogKey): T? = values.remove(key)

    @Synchronized
    fun removeIfSame(key: ForegroundWatchdogKey, value: T): Boolean {
        if (values[key] !== value) {
            return false
        }
        values.remove(key)
        return true
    }

    @Synchronized
    fun contains(key: ForegroundWatchdogKey): Boolean = values.containsKey(key)

    @Synchronized
    fun values(): List<T> = values.values.toList()

    @Synchronized
    fun clear() {
        values.clear()
    }
}
