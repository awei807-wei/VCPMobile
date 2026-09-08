package com.vcp.mobile.service

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.os.PowerManager
import android.util.Log

/** 进程级 WakeLock 与 WifiLock 的物理持有者。 */
internal interface ForegroundLockHandle {
    val isHeld: Boolean

    fun acquire()

    fun release()
}

internal class ForegroundPhysicalLocks(
    private val wakeLockFactory: (Context) -> ForegroundLockHandle? = { context ->
        createWakeLock(context)
    },
    private val wifiLockFactory: (Context) -> ForegroundLockHandle? = { context ->
        createWifiLock(context)
    },
    private val logDebug: (String) -> Unit = { message -> Log.d(TAG, message) },
    private val logError: (String, Throwable) -> Unit = { message, error ->
        Log.e(TAG, message, error)
    },
) {
    companion object {
        private const val TAG = "ForegroundGuardian"
    }

    private var wakeLock: ForegroundLockHandle? = null
    private var wifiLock: ForegroundLockHandle? = null

    fun acquire(context: Context) {
        try {
            val appContext = context.applicationContext
            acquireWakeLock(appContext)
            acquireWifiLock(appContext)
        } catch (error: Throwable) {
            logError("获取物理保活锁失败，正在回滚已获取的锁", error)
            release()
            throw error
        }
    }

    /** 返回是否仍有物理锁句柄处于持有状态，调用方可据此安排重试。 */
    fun release(): Boolean {
        wakeLock = releaseLock(wakeLock, "WakeLock")
        wifiLock = releaseLock(wifiLock, "WifiLock")
        return wakeLock != null || wifiLock != null
    }

    private fun acquireWakeLock(context: Context) {
        if (wakeLock == null) {
            wakeLock = wakeLockFactory(context)
        }
        wakeLock?.let {
            if (!it.isHeld) {
                it.acquire()
                logDebug("acquireLocks：WakeLock 已获取")
            }
        }
    }

    private fun acquireWifiLock(context: Context) {
        if (wifiLock == null) {
            wifiLock = wifiLockFactory(context)
        }
        wifiLock?.let {
            if (!it.isHeld) {
                it.acquire()
                logDebug("acquireLocks：WifiLock 已获取")
            }
        }
    }

    private fun releaseLock(
        lock: ForegroundLockHandle?,
        name: String,
    ): ForegroundLockHandle? {
        if (lock == null) return null
        try {
            if (!lock.isHeld) return null
            lock.release()
            if (!lock.isHeld) {
                logDebug("releaseLocks：$name 已释放")
                return null
            }
            logError(
                "releaseLocks：$name 释放后仍被持有，保留句柄以便重试",
                IllegalStateException("$name release 后 isHeld 仍为 true"),
            )
            return lock
        } catch (error: Throwable) {
            val stillHeld = runCatching { lock.isHeld }.getOrDefault(true)
            if (stillHeld) {
                logError("releaseLocks：$name 释放失败，保留句柄以便重试并继续其余锁", error)
                return lock
            }
            logError("releaseLocks：$name 释放抛错但已确认不再持有，继续其余锁", error)
        }
        return null
    }
}

private fun createWakeLock(context: Context): ForegroundLockHandle? {
    val powerManager = context.getSystemService(Context.POWER_SERVICE) as? PowerManager
    return powerManager?.newWakeLock(
        PowerManager.PARTIAL_WAKE_LOCK,
        "VCP:ForegroundGuardian",
    )?.let(::WakeLockHandle)
}

private fun createWifiLock(context: Context): ForegroundLockHandle? {
    val wifiManager = context.getSystemService(Context.WIFI_SERVICE) as? WifiManager
    if (wifiManager == null) return null
    @Suppress("DEPRECATION")
    val lock = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        wifiManager.createWifiLock(
            WifiManager.WIFI_MODE_FULL_HIGH_PERF,
            "VCP:ForegroundGuardianWifi",
        )
    } else {
        wifiManager.createWifiLock(
            WifiManager.WIFI_MODE_FULL,
            "VCP:ForegroundGuardianWifi",
        )
    }
    return WifiLockHandle(lock)
}

private class WakeLockHandle(
    private val delegate: PowerManager.WakeLock,
) : ForegroundLockHandle {
    override val isHeld: Boolean
        get() = delegate.isHeld

    override fun acquire() {
        delegate.acquire()
    }

    override fun release() {
        delegate.release()
    }
}

private class WifiLockHandle(
    private val delegate: WifiManager.WifiLock,
) : ForegroundLockHandle {
    override val isHeld: Boolean
        get() = delegate.isHeld

    override fun acquire() {
        delegate.acquire()
    }

    override fun release() {
        delegate.release()
    }
}
