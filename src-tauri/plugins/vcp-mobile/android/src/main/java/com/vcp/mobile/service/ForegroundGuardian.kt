package com.vcp.mobile.service

import android.content.Context
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import java.util.concurrent.ConcurrentHashMap

/**
 * 进程级前台守护者，统一协调 WakeLock、WifiLock 与前台服务生命周期。
 *
 * 消费者按 tag 幂等注册；通知展示最高优先级消费者。首次消费者只发起一次
 * startForegroundService，后续通知刷新改用普通 startService，避免重复创建前台提升契约。
 */
object ForegroundGuardian {
    private const val TAG = "ForegroundGuardian"
    private const val DISTRIBUTED_TAG = "distributed"

    const val PRIORITY_SYNC = 40
    const val PRIORITY_PRERENDER = 30
    const val PRIORITY_STREAM = 20
    const val PRIORITY_DISTRIBUTED = 10

    private val consumers = ConcurrentHashMap<String, ConsumerEntry>()
    private val handler = Handler(Looper.getMainLooper())
    private val timeoutRunnables = ConcurrentHashMap<String, Runnable>()

    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var foregroundStartPending = false

    data class ConsumerEntry(
        val priority: Int,
        val displayLabel: String,
        val screenKeepOn: Boolean
    )

    val isActive: Boolean
        get() = consumers.isNotEmpty()

    val isScreenKeepOnRequired: Boolean
        get() = consumers.values.any { it.screenKeepOn }

    fun getNotificationLabel(): String {
        return consumers.values.maxByOrNull { it.priority }?.displayLabel
            ?: "VCP 正在后台运行"
    }

    /** 注册或刷新一个前台任务消费者。 */
    @Synchronized
    fun acquire(
        context: Context,
        tag: String,
        priority: Int,
        label: String,
        screenKeepOn: Boolean = false,
        timeoutMs: Long = -1
    ) {
        Log.i(
            TAG,
            "acquire: tag=$tag, priority=$priority, label=$label, " +
                "screenKeepOn=$screenKeepOn, timeoutMs=$timeoutMs"
        )

        cancelTimeout(tag)
        val wasEmpty = consumers.isEmpty()
        consumers[tag] = ConsumerEntry(priority, label, screenKeepOn)

        if (tag == DISTRIBUTED_TAG) {
            StreamKeepaliveService.setDistributedKeepalivePersisted(context, true)
        }

        if (wasEmpty) {
            acquireLocks(context)
        }

        if (!ensureFgsStarted(context)) {
            clearRuntimeState()
            throw IllegalStateException("Unable to start StreamKeepaliveService as a foreground service")
        }

        scheduleTimeout(context, tag, timeoutMs)
    }

    /** 释放指定消费者；最后一个消费者退出时由 Service 串行完成自停。 */
    @Synchronized
    fun release(context: Context, tag: String) {
        Log.i(TAG, "release: tag=$tag")
        cancelTimeout(tag)

        if (tag == DISTRIBUTED_TAG) {
            StreamKeepaliveService.setDistributedKeepalivePersisted(context, false)
        }

        if (consumers.remove(tag) == null) {
            Log.d(TAG, "release: tag=$tag is not registered, ignore.")
            return
        }

        if (consumers.isEmpty()) {
            releaseLocks()
            reconcileFgsLifecycle(context)
        } else {
            updateFgs(context)
        }
    }

    /**
     * 从开机或包更新恢复 Intent 重建分布式消费者。
     *
     * 此方法只会在 StreamKeepaliveService 已经完成前台提升后调用，因此不会再次启动服务。
     */
    @Synchronized
    fun restoreDistributedConsumer(context: Context) {
        if (consumers.containsKey(DISTRIBUTED_TAG)) {
            return
        }

        Log.i(TAG, "Restoring persisted distributed foreground consumer.")
        consumers[DISTRIBUTED_TAG] = ConsumerEntry(
            PRIORITY_DISTRIBUTED,
            DISTRIBUTED_TAG,
            false
        )
        StreamKeepaliveService.setDistributedKeepalivePersisted(context, true)
        acquireLocks(context)
        scheduleTimeout(context, DISTRIBUTED_TAG, -1)
    }

    /** 显式停止全部前台任务，并清除分布式恢复意图。 */
    @Synchronized
    fun releaseAll(context: Context) {
        StreamKeepaliveService.setDistributedKeepalivePersisted(context, false)
        clearRuntimeState()
        reconcileFgsLifecycle(context)
    }

    /** 标记 Service 已在 onCreate 最早阶段完成前台提升。 */
    @Synchronized
    fun onServicePromoted() {
        foregroundStartPending = false
    }

    /** 前台提升同步失败时撤销运行态消费者，避免锁泄漏。 */
    @Synchronized
    fun onServicePromotionFailed() {
        foregroundStartPending = false
        clearRuntimeState()
    }

    /** Service 销毁时释放进程态锁；持久化恢复意图由显式 release 控制。 */
    @Synchronized
    fun onServiceDestroyed() {
        foregroundStartPending = false
        clearRuntimeState()
    }

    private fun scheduleTimeout(context: Context, tag: String, requestedTimeoutMs: Long) {
        val actualTimeout = if (requestedTimeoutMs >= 0) {
            requestedTimeoutMs
        } else {
            defaultTimeoutFor(tag)
        }

        if (actualTimeout <= 0) {
            return
        }

        val appContext = context.applicationContext
        val runnable = Runnable {
            Log.w(TAG, "Timeout reached for tag: $tag. Force releasing to prevent lock leak.")
            release(appContext, tag)
        }
        timeoutRunnables[tag] = runnable
        handler.postDelayed(runnable, actualTimeout)
        Log.d(TAG, "Scheduled timeout for tag: $tag in $actualTimeout ms")
    }

    private fun defaultTimeoutFor(tag: String): Long {
        return when {
            tag.startsWith("stream:") -> 10 * 60 * 1000L
            tag == "sync" -> 30 * 60 * 1000L
            tag == "prerender" -> 30 * 60 * 1000L
            tag == DISTRIBUTED_TAG || tag == "manual_keepalive" -> 2 * 60 * 60 * 1000L
            else -> 15 * 60 * 1000L
        }
    }

    private fun cancelTimeout(tag: String) {
        timeoutRunnables.remove(tag)?.let(handler::removeCallbacks)
    }

    private fun clearRuntimeState() {
        timeoutRunnables.values.forEach(handler::removeCallbacks)
        timeoutRunnables.clear()
        consumers.clear()
        releaseLocks()
    }

    private fun acquireLocks(context: Context) {
        val appContext = context.applicationContext

        if (wakeLock == null) {
            val powerManager = appContext.getSystemService(Context.POWER_SERVICE) as? PowerManager
            wakeLock = powerManager?.newWakeLock(
                PowerManager.PARTIAL_WAKE_LOCK,
                "VCP:ForegroundGuardian"
            )
        }
        wakeLock?.let {
            if (!it.isHeld) {
                it.acquire()
                Log.d(TAG, "acquireLocks: WakeLock acquired.")
            }
        }

        if (wifiLock == null) {
            val wifiManager = appContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            if (wifiManager != null) {
                @Suppress("DEPRECATION")
                wifiLock = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    wifiManager.createWifiLock(
                        WifiManager.WIFI_MODE_FULL_HIGH_PERF,
                        "VCP:ForegroundGuardianWifi"
                    )
                } else {
                    wifiManager.createWifiLock(
                        WifiManager.WIFI_MODE_FULL,
                        "VCP:ForegroundGuardianWifi"
                    )
                }
            }
        }
        wifiLock?.let {
            if (!it.isHeld) {
                it.acquire()
                Log.d(TAG, "acquireLocks: WifiLock acquired.")
            }
        }
    }

    private fun releaseLocks() {
        wakeLock?.let {
            if (it.isHeld) {
                it.release()
                Log.d(TAG, "releaseLocks: WakeLock released.")
            }
        }
        wakeLock = null

        wifiLock?.let {
            if (it.isHeld) {
                it.release()
                Log.d(TAG, "releaseLocks: WifiLock released.")
            }
        }
        wifiLock = null
    }

    private fun ensureFgsStarted(context: Context): Boolean {
        if (StreamKeepaliveService.isServiceRunning) {
            updateFgs(context)
            return true
        }

        if (foregroundStartPending) {
            Log.d(TAG, "Foreground start already pending; coalescing duplicate request.")
            return true
        }

        return startFgs(context)
    }

    private fun startFgs(context: Context): Boolean {
        val appContext = context.applicationContext
        val intent = Intent(appContext, StreamKeepaliveService::class.java)
        foregroundStartPending = true
        Log.i(TAG, "startFgs: Starting StreamKeepaliveService...")

        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                appContext.startForegroundService(intent)
            } else {
                appContext.startService(intent)
            }
            true
        } catch (e: Exception) {
            foregroundStartPending = false
            Log.e(TAG, "startFgs failed", e)
            false
        }
    }

    private fun updateFgs(context: Context) {
        if (!StreamKeepaliveService.isServiceRunning) {
            ensureFgsStarted(context)
            return
        }

        val appContext = context.applicationContext
        val intent = Intent(appContext, StreamKeepaliveService::class.java).apply {
            action = StreamKeepaliveService.ACTION_REFRESH_NOTIFICATION
        }
        try {
            // 已运行的前台服务只需普通 startService 更新，不能重复制造前台提升计时契约。
            appContext.startService(intent)
        } catch (e: Exception) {
            Log.e(TAG, "updateFgs failed", e)
        }
    }

    private fun reconcileFgsLifecycle(context: Context) {
        if (!StreamKeepaliveService.isServiceRunning) {
            // 若首次 startForegroundService 尚在排队，不能 stopService 抢先取消；
            // Service 会先在 onCreate 完成提升，再在 onStartCommand 发现空消费者后自停。
            if (foregroundStartPending) {
                Log.d(TAG, "Foreground start is pending; deferring stop until service reconciliation.")
            }
            return
        }

        val appContext = context.applicationContext
        val intent = Intent(appContext, StreamKeepaliveService::class.java).apply {
            action = StreamKeepaliveService.ACTION_RECONCILE_LIFECYCLE
        }
        try {
            appContext.startService(intent)
        } catch (e: Exception) {
            Log.e(TAG, "Failed to reconcile StreamKeepaliveService lifecycle", e)
        }
    }
}
