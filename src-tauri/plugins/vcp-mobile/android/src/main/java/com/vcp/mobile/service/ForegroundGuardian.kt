package com.vcp.mobile.service

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.util.concurrent.ConcurrentHashMap

/**
 * 进程级前台守护者，统一协调 WakeLock、WifiLock 与前台服务生命周期。
 *
 * 消费者按 tag 幂等注册；通知展示最高优先级消费者。应用可见时只登记消费者，
 * 进入后台后才发起一次 startForegroundService，后续刷新使用普通 startService。
 */
object ForegroundGuardian {
    private const val TAG = "ForegroundGuardian"
    private const val DISTRIBUTED_TAG = ForegroundTagContract.DISTRIBUTED_TAG

    const val PRIORITY_SYNC = 40
    const val PRIORITY_PRERENDER = 30
    const val PRIORITY_STREAM = 20
    const val PRIORITY_DISTRIBUTED = 10

    private val consumers = ConcurrentHashMap<String, ConsumerEntry>()
    private val streamLeases = ForegroundLeaseLedger()
    private val handler = Handler(Looper.getMainLooper())
    private val watchdog = ForegroundWatchdog(handler) { context, tag, generation ->
        Log.w(TAG, "前台 lease 超时：category=${categoryForTag(tag)}，result=release")
        release(context, tag, generation)
    }

    private val physicalLocks = ForegroundPhysicalLocks()
    private val physicalLockReleaseScheduler = ForegroundLockReleaseScheduler(
        postDelayed = { runnable, delay -> handler.postDelayed(runnable, delay) },
        removeCallbacks = { runnable -> handler.removeCallbacks(runnable) },
        releaseLocks = { physicalLocks.release() },
        logError = { message -> Log.e(TAG, message) },
    )
    private val serviceLifecycle = ForegroundServiceLifecycleCoordinator { appInForeground }
    private val failureCoordinator = ForegroundFailureCoordinator(
        clearRuntimeState = { clearRuntimeState() },
        clearPersistence = { context ->
            StreamKeepaliveService.setDistributedKeepalivePersisted(context, false)
        },
        reconcileService = { context -> serviceLifecycle.reconcile(context) },
    )
    private var appInForeground = true

    data class ConsumerEntry(
        val priority: Int,
        val displayLabel: String,
        val screenKeepOn: Boolean
    )

    val isActive: Boolean
        get() = consumers.isNotEmpty()

    val isScreenKeepOnRequired: Boolean
        get() = consumers.values.any { it.screenKeepOn }

    /** 为一个完整流身份生成抗碰撞 lease tag。 */
    fun streamTag(key: StreamSessionKey): String = StreamLeaseTag.forKey(key)

    /** 没有所有权字段的旧调用方继续使用稳定且带命名空间的 tag。 */
    fun legacyStreamTag(agentName: String): String = StreamLeaseTag.forLegacyAgent(agentName)

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
    ): Long? {
        Log.i(
            TAG,
            "acquire: category=${categoryForPriority(priority)}, priority=$priority, " +
                "screenKeepOn=$screenKeepOn, consumers=${consumers.size}, result=accepted"
        )

        if (ForegroundTagContract.isGenerationDistributedTag(tag)) {
            removeLegacyDistributedPlaceholder()
        }
        val leaseGeneration = if (isIdentityStreamTag(tag)) {
            streamLeases.acquire(tag)
        } else {
            null
        }
        consumers[tag] = ConsumerEntry(priority, label, screenKeepOn)
        updateDistributedPersistence(context)

        physicalLockReleaseScheduler.cancel()
        try {
            physicalLocks.acquire(context)
        } catch (error: Throwable) {
            clearFailedForegroundState(context)
            Log.e(TAG, "获取前台物理锁失败，已撤销消费者", error)
            throw IllegalStateException("无法获取前台保活锁", error)
        }

        if (StreamKeepaliveService.isServiceRunning) {
            if (!serviceLifecycle.update(context)) {
                clearFailedForegroundState(context)
                throw IllegalStateException("无法刷新 StreamKeepaliveService 前台状态")
            }
        } else if (!appInForeground && !serviceLifecycle.ensureStarted(context)) {
            // 后台提升失败时不再调和物理锁；失败服务没有所有者，必须清空全部共享状态。
            clearFailedForegroundState(context)
            throw IllegalStateException("无法将 StreamKeepaliveService 启动为前台服务")
        } else if (appInForeground) {
            Log.d(TAG, "应用当前位于前台，将前台服务启动延迟到进入后台")
        }

        if (!isIdentityStreamTag(tag)) {
            cancelTimeout(tag, null)
        }
        scheduleTimeout(context, tag, timeoutMs, leaseGeneration)
        return leaseGeneration
    }

    /** 释放指定消费者；最后一个消费者退出时由 Service 串行完成自停。 */
    @Synchronized
    fun release(context: Context, tag: String, expectedGeneration: Long? = null) {
        val isStreamLease = isIdentityStreamTag(tag)
        if (isStreamLease && expectedGeneration == null) {
            Log.e(TAG, "release: category=stream, result=missing_generation")
            return
        }
        val releasedGeneration = if (isStreamLease) {
            streamLeases.releaseGeneration(tag, expectedGeneration)
        } else {
            null
        }
        if (isStreamLease && releasedGeneration == null) {
            Log.d(TAG, "release: category=stream, result=stale_generation")
            return
        }
        cancelTimeout(tag, releasedGeneration)
        if (isStreamLease && streamLeases.isHeld(tag)) {
            if (StreamKeepaliveService.isServiceRunning || !appInForeground) {
                if (!serviceLifecycle.update(context)) {
                    clearFailedForegroundState(context)
                }
            }
            return
        }
        if (consumers.remove(tag) == null) {
            if (ForegroundTagContract.isDistributedTag(tag)) {
                updateDistributedPersistence(context)
            }
            Log.d(TAG, "release: category=${categoryForTag(tag)}, result=not_registered")
            return
        }
        updateDistributedPersistence(context)

        if (consumers.isEmpty()) {
            physicalLockReleaseScheduler.release()
            serviceLifecycle.reconcile(context)
        } else if (StreamKeepaliveService.isServiceRunning || !appInForeground) {
            if (!serviceLifecycle.update(context)) {
                clearFailedForegroundState(context)
            }
        }
        Log.i(
            TAG,
            "release: category=${categoryForTag(tag)}, consumers=${consumers.size}, result=accepted"
        )
    }

    /**
     * 同步应用可见状态。应用可见时不创建前台服务；只有存在消费者并真正进入后台时
     * 才发起 startForegroundService，避免冷启动主线程拥塞耗尽系统提升时限。
     */
    @Synchronized
    fun onAppForegroundChanged(context: Context, isForeground: Boolean) {
        appInForeground = isForeground
        Log.d(TAG, "onAppForegroundChanged: foreground=$isForeground, consumers=${consumers.size}")

        if (isForeground || consumers.isEmpty()) {
            return
        }

        physicalLockReleaseScheduler.cancel()
        try {
            physicalLocks.acquire(context)
        } catch (error: Throwable) {
            clearFailedForegroundState(context)
            Log.e(TAG, "后台获取前台物理锁失败，已撤销消费者", error)
            return
        }
        if (!serviceLifecycle.ensureStarted(context)) {
            // 后台启动被系统拒绝时撤销共享消费者，避免无通知服务继续持有物理锁。
            clearFailedForegroundState(context)
            Log.e(TAG, "后台前台服务启动失败，已撤销消费者并释放保活锁")
        }
    }

    /**
     * 从开机或包更新恢复 Intent 重建分布式消费者。
     *
     * 此方法只会在 StreamKeepaliveService 已经完成前台提升后调用，因此不会再次启动服务。
     */
    @Synchronized
    fun restoreDistributedConsumer(context: Context) {
        if (consumers.keys.any(ForegroundTagContract::isDistributedTag)) {
            return
        }

        Log.i(TAG, "恢复已持久化的分布式前台消费者")
        appInForeground = false
        consumers[DISTRIBUTED_TAG] = ConsumerEntry(
            PRIORITY_DISTRIBUTED,
            DISTRIBUTED_TAG,
            false
        )
        updateDistributedPersistence(context)
        physicalLockReleaseScheduler.cancel()
        try {
            physicalLocks.acquire(context)
        } catch (error: Throwable) {
            clearFailedForegroundState(context)
            Log.e(TAG, "恢复分布式前台物理锁失败，已撤销消费者", error)
            return
        }
        scheduleTimeout(context, DISTRIBUTED_TAG, -1, null)
    }

    /** 显式停止全部前台任务，并清除分布式恢复意图。 */
    @Synchronized
    fun releaseAll(context: Context) {
        StreamKeepaliveService.setDistributedKeepalivePersisted(context, false)
        clearRuntimeState()
        serviceLifecycle.reconcile(context)
    }

    /**
     * 收口启动恢复留下的全部分布式消费者，但保留独立的流式消费者。
     *
     * Rust bootstrap 读取数据库中的权威设置后可能发现 Android BootReceiver
     * 依据旧 SharedPreferences 恢复了分布式占位消费者。此时不能只释放
     * `distributed` 这个旧 tag：连接代际和派生任务也可能留下带前缀的
     * native lease。统一按分布式命名空间清除，随后再由 Service 串行释放
     * 音频、物理锁和前台通知。
     */
    @Synchronized
    fun releaseDistributed(context: Context) {
        var removed = false
        consumers.keys
            .filter(ForegroundTagContract::isDistributedTag)
            .toList()
            .forEach { tag ->
                cancelTimeout(tag, null)
                removed = consumers.remove(tag) != null || removed
            }

        // 即使运行时没有对应 consumer，也要清除新旧两套持久化键，避免
        // 下次 BOOT_COMPLETED 再次按过期意图拉起服务。
        updateDistributedPersistence(context)
        if (consumers.isEmpty()) {
            physicalLockReleaseScheduler.release()
            serviceLifecycle.reconcile(context)
        } else if (StreamKeepaliveService.isServiceRunning || !appInForeground) {
            if (!serviceLifecycle.update(context)) {
                clearFailedForegroundState(context)
            }
        }
        Log.i(
            TAG,
            "清理分布式前台恢复状态：removed=$removed, consumers=${consumers.size}, " +
                "result=accepted",
        )
    }

    /** 标记 Service 已在 onCreate 最早阶段完成前台提升。 */
    @Synchronized
    fun onServicePromoted() {
        serviceLifecycle.onServicePromoted()
    }

    /** 前台提升失败后撤销共享消费者与物理锁，避免失败服务留下无主保活状态。 */
    @Synchronized
    fun onServicePromotionFailed(context: Context) {
        serviceLifecycle.onServiceDestroyed()
        clearFailedForegroundState(context)
    }

    /** Service 销毁时释放物理锁但保留业务消费者，避免服务异常退出篡改连接状态。 */
    @Synchronized
    fun onServiceDestroyed() {
        serviceLifecycle.onServiceDestroyed()
        physicalLockReleaseScheduler.release()
    }

    private fun scheduleTimeout(
        context: Context,
        tag: String,
        requestedTimeoutMs: Long,
        leaseGeneration: Long?
    ) {
        val actualTimeout = if (requestedTimeoutMs >= 0) {
            requestedTimeoutMs
        } else {
            ForegroundTimeoutPolicy.forTag(tag)
        }

        if (actualTimeout <= 0) {
            return
        }

        watchdog.schedule(context.applicationContext, tag, leaseGeneration, actualTimeout)
        Log.d(
            TAG,
            "timeout scheduled: category=${categoryForTag(tag)}, consumers=${consumers.size}, result=accepted"
        )
    }

    private fun cancelTimeout(tag: String, generation: Long?) {
        watchdog.cancel(tag, generation)
    }

    private fun removeLegacyDistributedPlaceholder() {
        if (consumers.remove(DISTRIBUTED_TAG) != null) {
            cancelTimeout(DISTRIBUTED_TAG, null)
            Log.i(TAG, "移除旧分布式占位消费者：category=distributed, result=accepted")
        }
    }

    private fun updateDistributedPersistence(context: Context) {
        StreamKeepaliveService.setDistributedKeepalivePersisted(
            context,
            consumers.keys.any(ForegroundTagContract::isDistributedTag),
        )
    }

    private fun clearRuntimeState() {
        watchdog.clear()
        consumers.clear()
        streamLeases.clear()
        physicalLockReleaseScheduler.release()
    }

    private fun clearFailedForegroundState(context: Context) {
        failureCoordinator.clear(context)
    }
}
