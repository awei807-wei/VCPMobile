package com.vcp.mobile.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat

/**
 * 流式响应前台保活服务
 *
 * 当 Agent 正在流式生成回复时启动，通过持续通知向系统声明"用户感知的重要任务"，
 * 显著降低进程被 OEM 杀后台的概率。
 *
 * 设计原则：高可见性常驻保活
 * - 通知使用 IMPORTANCE_HIGH 确保在所有 OEM（ColorOS/EMUI/HarmonyOS/MIUI）上显式显示
 * - 服务运行期间通知常驻通知栏，不可滑动关闭
 * - 流结束立即自毁，绝不空占
 *
 * 注意：本服务已瘦身为 ForegroundGuardian 的前台通知载体，
 * 双锁 (WakeLock + WifiLock) 的生命周期由 ForegroundGuardian 统一管理。
 */
class StreamKeepaliveService : Service() {

    companion object {
        const val CHANNEL_ID = "vcp_stream_keepalive"
        const val NOTIFICATION_ID = 0x53545201 // "STR" + 01
        const val ACTION_REFRESH_NOTIFICATION = "com.vcp.mobile.action.REFRESH_KEEPALIVE_NOTIFICATION"
        const val ACTION_RECONCILE_LIFECYCLE = "com.vcp.mobile.action.RECONCILE_KEEPALIVE_LIFECYCLE"
        const val ACTION_RECOVER_KEEPALIVE = "com.vcp.mobile.action.RECOVER_KEEPALIVE"
        private const val TAG = "VcpMobileService"
        private const val PREFS_NAME = "vcp_mobile_keepalive"
        private const val PREF_DISTRIBUTED_KEEPALIVE = "distributed_keepalive_active"
        private const val COMPAT_PREFS_NAME = "vcp_keepalive_prefs"
        private const val COMPAT_PREF_DISTRIBUTED_KEEPALIVE = "distributed_keepalive_persisted"
        private const val EXTRA_RECOVERY_MODE = "recovery_mode"

        @Volatile
        var isServiceRunning = false

        /**
         * Check if distributed keepalive was requested before boot/reboot.
         */
        fun isDistributedKeepalivePersisted(context: Context): Boolean {
            val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            if (prefs.contains(PREF_DISTRIBUTED_KEEPALIVE)) {
                return prefs.getBoolean(PREF_DISTRIBUTED_KEEPALIVE, false)
            }

            return context
                .getSharedPreferences(COMPAT_PREFS_NAME, Context.MODE_PRIVATE)
                .getBoolean(COMPAT_PREF_DISTRIBUTED_KEEPALIVE, false)
        }

        /**
         * Set/clear the distributed keepalive persisted flag.
         */
        fun setDistributedKeepalivePersisted(context: Context, enabled: Boolean) {
            context
                .getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .edit()
                .putBoolean(PREF_DISTRIBUTED_KEEPALIVE, enabled)
                .apply()

            // 同步维护 1.0.7 曾使用过的键，避免版本切换后恢复意图丢失。
            context
                .getSharedPreferences(COMPAT_PREFS_NAME, Context.MODE_PRIVATE)
                .edit()
                .putBoolean(COMPAT_PREF_DISTRIBUTED_KEEPALIVE, enabled)
                .apply()
        }

        /**
         * Create an Intent for best-effort recovery after boot.
         */
        fun createRecoveryIntent(context: Context): Intent {
            return Intent(context, StreamKeepaliveService::class.java).apply {
                action = ACTION_RECOVER_KEEPALIVE
                putExtra(EXTRA_RECOVERY_MODE, true)
            }
        }

        private fun isRecoveryIntent(intent: Intent?): Boolean {
            return intent?.action == ACTION_RECOVER_KEEPALIVE ||
                intent?.getBooleanExtra(EXTRA_RECOVERY_MODE, false) == true
        }
    }

    private var mediaPlayer: MediaPlayer? = null
    private var foregroundPromoted = false

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()

        // startForegroundService() 的超时从调用方发起时就开始计时。这里在 onCreate 的
        // 最早可用阶段先发布最小通知，业务文案和音频初始化留到 onStartCommand 处理。
        foregroundPromoted = promoteToForeground(buildBootstrapNotification())
        if (!foregroundPromoted) {
            Log.e(TAG, "Bootstrap foreground promotion failed; stopping service immediately.")
            ForegroundGuardian.onServicePromotionFailed()
            stopSelf()
            return
        }

        isServiceRunning = true
        ForegroundGuardian.onServicePromoted()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (!foregroundPromoted) {
            foregroundPromoted = promoteToForeground(buildBootstrapNotification())
            if (!foregroundPromoted) {
                Log.e(TAG, "Foreground promotion unavailable in onStartCommand; stopping service.")
                ForegroundGuardian.onServicePromotionFailed()
                stopSelf(startId)
                return START_NOT_STICKY
            }
            isServiceRunning = true
            ForegroundGuardian.onServicePromoted()
        }

        if (isRecoveryIntent(intent) && isDistributedKeepalivePersisted(this)) {
            ForegroundGuardian.restoreDistributedConsumer(this)
        }

        // 停止也通过 Service 主线程串行化处理：若停止请求后紧接着有新消费者进入，
        // 最新 startId 会阻止旧停止请求误杀刚恢复的前台服务。
        if (!ForegroundGuardian.isActive) {
            Log.i(TAG, "No foreground consumers remain; stopping after fulfilling foreground contract.")
            stopSelf(startId)
            return START_NOT_STICKY
        }

        val label = ForegroundGuardian.getNotificationLabel()
        val notification = buildNotification(label)

        // 此时最小通知已经完成前台提升；这里仅刷新业务文案。
        if (!promoteToForeground(notification)) {
            Log.w(TAG, "Notification refresh failed; keeping bootstrap foreground notification.")
        }

        // 启动静音音频循环播放保活
        startSilentPlayback()

        return START_NOT_STICKY
    }

    /**
     * 提升为前台服务，包含异常兜底。
     * 保留自 fix/keepalive-service-crash 分支的改进。
     */
    private fun promoteToForeground(notification: Notification): Boolean {
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                startForeground(
                    NOTIFICATION_ID,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
                )
            } else {
                startForeground(NOTIFICATION_ID, notification)
            }
            true
        } catch (e: Exception) {
            Log.e(TAG, "startForeground failed", e)
            false
        }
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
        if (!isDistributedKeepalivePersisted(this) || !ForegroundGuardian.isActive) {
            return
        }

        // stopWithTask=false 时服务仍在运行，不再递归调用 startForegroundService，
        // 只重申现有前台通知，避免制造新的五秒提升契约与重复启动竞态。
        Log.i(TAG, "Task removed while distributed keepalive is active; retaining current foreground service.")
        promoteToForeground(buildNotification(ForegroundGuardian.getNotificationLabel()))
    }

    override fun onDestroy() {
        if (foregroundPromoted) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                stopForeground(STOP_FOREGROUND_REMOVE)
            } else {
                @Suppress("DEPRECATION")
                stopForeground(true)
            }
        }
        
        // 停止静音音频播放
        stopSilentPlayback()
        
        foregroundPromoted = false
        isServiceRunning = false
        // 服务销毁（包括系统回收）时释放进程级物理锁，但保留已持久化的分布式用户意图。
        ForegroundGuardian.onServiceDestroyed()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    // ==================================================================
    // Silent Audio Keep-Alive
    // ==================================================================

    private fun ensureSilentAudioFile(): java.io.File {
        val file = java.io.File(cacheDir, "silent.wav")
        if (file.exists() && file.length() > 0) {
            return file
        }
        try {
            file.outputStream().use { out ->
                // RIFF Header
                out.write(byteArrayOf(0x52, 0x49, 0x46, 0x46)) // "RIFF"
                out.write(byteArrayOf(0x64, 0x06, 0x00, 0x00)) // Size: 1636
                out.write(byteArrayOf(0x57, 0x41, 0x56, 0x45)) // "WAVE"
                
                // fmt Chunk
                out.write(byteArrayOf(0x66, 0x6d, 0x74, 0x20)) // "fmt "
                out.write(byteArrayOf(0x10, 0x00, 0x00, 0x00)) // Chunk size: 16
                out.write(byteArrayOf(0x01, 0x00))             // Format: 1 (PCM)
                out.write(byteArrayOf(0x01, 0x00))             // Channels: 1 (Mono)
                out.write(byteArrayOf(0x40, 0x1F, 0x00, 0x00)) // Sample rate: 8000
                out.write(byteArrayOf(0x40, 0x1F, 0x00, 0x00)) // Byte rate: 8000
                out.write(byteArrayOf(0x01, 0x00))             // Block align: 1
                out.write(byteArrayOf(0x08, 0x00))             // Bits per sample: 8
                
                // data Chunk
                out.write(byteArrayOf(0x64, 0x61, 0x74, 0x61)) // "data"
                out.write(byteArrayOf(0x40, 0x06, 0x00, 0x00)) // Data size: 1600
                
                // 1600 bytes of silence (0x80 for 8-bit PCM)
                val silence = ByteArray(1600) { 0x80.toByte() }
                out.write(silence)
            }
            Log.i(TAG, "Created silent.wav in cache directory.")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to create silent.wav", e)
        }
        return file
    }

    private fun startSilentPlayback() {
        if (mediaPlayer != null) return
        try {
            val silentFile = ensureSilentAudioFile()
            mediaPlayer = MediaPlayer().apply {
                setDataSource(silentFile.absolutePath)
                isLooping = true
                
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                    setAudioAttributes(
                        AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                            .build()
                    )
                }
                
                prepare()
                start()
            }
            Log.i(TAG, "Silent playback started.")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start silent playback", e)
        }
    }

    private fun stopSilentPlayback() {
        mediaPlayer?.let {
            try {
                if (it.isPlaying) {
                    it.stop()
                }
                it.release()
            } catch (ignored: Exception) {}
        }
        mediaPlayer = null
        Log.i(TAG, "Silent playback stopped.")
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "后台服务增强通道",
                NotificationManager.IMPORTANCE_HIGH
            ).apply {
                description = "Agent 流式响应与后台保活"
                setShowBadge(false)
                enableVibration(false)
                setSound(null, null)
            }
            getSystemService(NotificationManager::class.java)
                ?.createNotificationChannel(channel)
        }
    }

    private fun buildBootstrapNotification(): Notification {
        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("VCP Mobile")
            .setContentText("正在准备后台任务…")
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .setOnlyAlertOnce(true)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }

    private fun buildNotification(label: String): Notification {
        // 点击通知：打开应用（通过反射获取主 Activity，避免跨包编译依赖）
        val openIntent = try {
            val mainActivityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(this, mainActivityClass).apply {
                flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            }
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }
        val openPendingIntent = PendingIntent.getActivity(
            this, 0, openIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val contentText = when {
            label.contains("[数据同步]") -> "正在与云端服务器进行高精度同步..."
            label.contains("[预渲染重建]") -> "正在优化与加速本地响应缓存..."
            label == "distributed" || label.contains("分布式") -> "分布式后台连接维系中..."
            label == "[后台保活]" -> "后台保活连接维系中..."
            label.isNotEmpty() -> "思考中……"
            else -> "已连接"
        }
        val cleanTitle = label.replace("[数据同步]", "").replace("[预渲染重建]", "").trim()
        val title = if (cleanTitle.isEmpty() || cleanTitle == "distributed" || cleanTitle == "[后台保活]") "VCP Mobile" else cleanTitle

        val builder = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(title)
            .setContentText(contentText)
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openPendingIntent)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }

        return builder.build()
    }
}
