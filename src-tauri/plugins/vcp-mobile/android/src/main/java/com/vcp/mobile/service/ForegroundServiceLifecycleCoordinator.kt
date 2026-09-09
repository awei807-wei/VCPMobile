package com.vcp.mobile.service

import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log

/** 集中管理 StreamKeepaliveService 的启动、刷新和生命周期收口。 */
internal class ForegroundServiceLifecycleCoordinator(
    private val appInForeground: () -> Boolean,
) {
    companion object {
        private const val TAG = "ForegroundGuardian"
    }

    private var startPending = false

    fun onServicePromoted() {
        startPending = false
    }

    fun onServiceDestroyed() {
        startPending = false
    }

    fun ensureStarted(context: Context): Boolean {
        if (StreamKeepaliveService.isServiceRunning) {
            return update(context)
        }
        if (startPending) {
            Log.d(TAG, "前台服务启动请求已排队，合并重复请求")
            return true
        }
        return start(context)
    }

    fun update(context: Context): Boolean {
        if (!StreamKeepaliveService.isServiceRunning) {
            if (!appInForeground()) {
                return ensureStarted(context)
            }
            return true
        }

        val appContext = context.applicationContext
        val intent = Intent(appContext, StreamKeepaliveService::class.java).apply {
            action = StreamKeepaliveService.ACTION_REFRESH_NOTIFICATION
        }
        return try {
            // 已运行的前台服务只需普通 startService 更新，不能重复制造前台提升计时契约。
            appContext.startService(intent)
            true
        } catch (error: Exception) {
            Log.e(TAG, "updateFgs 失败", error)
            false
        }
    }

    fun reconcile(context: Context) {
        if (!StreamKeepaliveService.isServiceRunning) {
            // 若首次 startForegroundService 尚在排队，不能 stopService 抢先取消；
            // Service 会先在 onCreate 完成提升，再在 onStartCommand 发现空消费者后自停。
            if (startPending) {
                Log.d(TAG, "前台服务启动仍在排队，延迟停止直到服务完成生命周期协调")
            }
            return
        }

        val appContext = context.applicationContext
        val intent = Intent(appContext, StreamKeepaliveService::class.java).apply {
            action = StreamKeepaliveService.ACTION_RECONCILE_LIFECYCLE
        }
        try {
            appContext.startService(intent)
        } catch (error: Exception) {
            Log.e(TAG, "协调 StreamKeepaliveService 生命周期失败", error)
            try {
                appContext.stopService(Intent(appContext, StreamKeepaliveService::class.java))
            } catch (stopError: Exception) {
                Log.e(TAG, "强制停止 StreamKeepaliveService 失败", stopError)
            }
        }
    }

    private fun start(context: Context): Boolean {
        val appContext = context.applicationContext
        val intent = Intent(appContext, StreamKeepaliveService::class.java)
        startPending = true
        Log.i(TAG, "startFgs：启动 StreamKeepaliveService")

        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                appContext.startForegroundService(intent)
            } else {
                appContext.startService(intent)
            }
            true
        } catch (error: Exception) {
            startPending = false
            Log.e(TAG, "startFgs 失败", error)
            false
        }
    }
}
