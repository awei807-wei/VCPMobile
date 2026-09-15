package com.vcp.mobile

import android.app.Activity
import android.content.Context
import android.content.IntentFilter
import android.content.res.Configuration
import android.os.Build
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.TauriPlugin
import androidx.activity.result.ActivityResult
import app.tauri.plugin.Plugin
import android.content.Intent
import android.content.ComponentName
import android.util.Log
import androidx.core.content.FileProvider
import android.webkit.MimeTypeMap
import android.media.AudioAttributes
import android.os.PowerManager
import android.net.Uri
import android.provider.Settings
import android.content.pm.PackageManager
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.plugin.JSObject
import app.tauri.plugin.JSArray
import app.tauri.plugin.Invoke
import android.graphics.Bitmap
import android.graphics.Canvas
import android.content.ContentValues
import android.provider.MediaStore
import android.os.Environment
import android.media.MediaScannerConnection
import android.util.Base64
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLDecoder
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.atomic.AtomicInteger
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import com.topjohnwu.superuser.Shell
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.transformer.Transformer
import androidx.media3.transformer.TransformationRequest
import androidx.media3.transformer.ExportException
import androidx.media3.transformer.ExportResult
import androidx.media3.transformer.EditedMediaItem
import androidx.media3.transformer.Composition
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

open class VcpMobileNotificationCommands(activity: Activity) : VcpMobileMediaCommands(activity) {
    protected var downloadNotificationBuilder: androidx.core.app.NotificationCompat.Builder? = null
    protected val DOWNLOAD_NOTIF_ID = 0x53545209
    protected val DOWNLOAD_CHANNEL_ID = "apk_download"
    protected val agentNotificationIdCounter = AtomicInteger((System.nanoTime() and AGENT_MESSAGE_NOTIF_SEQUENCE_MASK.toLong()).toInt())
    private val agentNotificationBurstGate = AgentNotificationBurstGate()
    private val agentMessageNotificationFactory = AgentMessageNotificationFactory(
        activity,
        AGENT_MESSAGE_CHANNEL_ID,
        AGENT_MESSAGE_GROUP_KEY
    )

    private companion object {
        const val AGENT_MESSAGE_GROUP_KEY = "com.vcp.mobile.AGENT_MESSAGES"
        const val AGENT_MESSAGE_SUMMARY_ID = -0x41474D
    }

    protected fun nextAgentMessageNotificationId(): Int {
        val sequence = agentNotificationIdCounter.incrementAndGet() and AGENT_MESSAGE_NOTIF_SEQUENCE_MASK
        return AGENT_MESSAGE_NOTIF_BASE_ID xor sequence
    }

    protected fun createDownloadNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val name = "应用更新下载"
            val descriptionText = "显示 APK 安装包的下载进度"
            val importance = android.app.NotificationManager.IMPORTANCE_LOW
            val channel = android.app.NotificationChannel(DOWNLOAD_CHANNEL_ID, name, importance).apply {
                description = descriptionText
            }
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.createNotificationChannel(channel)
        }
    }
    @Command
    fun startDownloadNotification(invoke: Invoke) {
        try {
            createDownloadNotificationChannel()
            val builder = androidx.core.app.NotificationCompat.Builder(activity, DOWNLOAD_CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_sys_download)
                .setContentTitle("正在下载 VCP Mobile 更新...")
                .setContentText("已下载 0%")
                .setOngoing(true)
                .setProgress(100, 0, false)
                .setOnlyAlertOnce(true)

            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.notify(DOWNLOAD_NOTIF_ID, builder.build())
            downloadNotificationBuilder = builder
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "启动下载通知失败", e)
            invoke.reject(e.message ?: "启动下载通知失败")
        }
    }

    @Command
    fun updateDownloadNotification(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(UpdateDownloadNotifArgs::class.java)
            val progress = args.progress
            val text = args.text ?: "正在下载..."

            val builder = downloadNotificationBuilder
            if (builder != null) {
                builder.setProgress(100, progress, false)
                    .setContentText(text)
                val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
                notificationManager.notify(DOWNLOAD_NOTIF_ID, builder.build())
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "更新下载通知失败", e)
            invoke.reject(e.message ?: "更新下载通知失败")
        }
    }

    @Command
    fun showSystemNotification(invoke: Invoke) {
        try {
            if (!checkNotificationPermission(invoke)) return
            val args = invoke.parseArgs(ShowSystemNotificationArgs::class.java)
            val title = args.title.ifBlank { "Agent 消息" }.take(120)
            val body = args.body.ifBlank { "收到一条新消息" }.take(3000)
            val now = System.currentTimeMillis()
            val notificationKey = "$title\n$body"
            val notificationDecision = agentNotificationBurstGate.evaluate(notificationKey, now)
            if (notificationDecision.skipDuplicate) {
                Log.i(TAG, "显示系统通知已跳过 5 秒内重复的 AgentMessage")
                invoke.resolve()
                return
            }
            createAgentMessageNotificationChannel()
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            val notificationId = nextAgentMessageNotificationId()
            if (notificationDecision.startsNewBurst) {
                notificationManager.cancel(AGENT_MESSAGE_SUMMARY_ID)
            }
            notificationManager.notify(
                notificationId,
                agentMessageNotificationFactory.buildMessage(title, body, now),
            )
            val groupedCount = max(
                1,
                notificationManager.activeNotifications.count { activeNotification ->
                    activeNotification.id != AGENT_MESSAGE_SUMMARY_ID &&
                        activeNotification.notification.group == AGENT_MESSAGE_GROUP_KEY
                }
            )
            notificationManager.notify(
                AGENT_MESSAGE_SUMMARY_ID,
                agentMessageNotificationFactory.buildSummary(title, body, groupedCount, now),
            )
            Log.i(
                TAG,
                "系统通知已分组发布：id=$notificationId，channel=$AGENT_MESSAGE_CHANNEL_ID，groupedCount=$groupedCount，startsNewBurst=${notificationDecision.startsNewBurst}，正文长度=${body.length}"
            )
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "显示系统通知失败", e)
            invoke.reject(e.message ?: "显示系统通知失败")
        }
    }

    private fun checkNotificationPermission(invoke: Invoke): Boolean {
        if (!androidx.core.app.NotificationManagerCompat.from(activity).areNotificationsEnabled()) {
            Log.w(TAG, "显示系统通知被拒绝：应用通知已关闭，package=${activity.packageName}")
            invoke.reject("当前应用的通知已关闭")
            return false
        }
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(
                activity,
                android.Manifest.permission.POST_NOTIFICATIONS,
            ) != PackageManager.PERMISSION_GRANTED
        ) {
            Log.w(TAG, "显示系统通知被拒绝：未授予 POST_NOTIFICATIONS 权限")
            invoke.reject("未授予 POST_NOTIFICATIONS 权限")
            return false
        }
        return true
    }

    @Command
    fun cancelDownloadNotification(invoke: Invoke) {
        try {
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.cancel(DOWNLOAD_NOTIF_ID)
            downloadNotificationBuilder = null
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "取消下载通知失败", e)
            invoke.reject(e.message ?: "取消下载通知失败")
        }
    }

    @Command
    fun startHelperService(invoke: Invoke) {
        try {
            startHelperServiceInternal()
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "启动辅助服务失败", e)
            invoke.reject(e.message ?: "启动辅助服务失败")
        }
    }

    @Command
    fun getPendingNotification(invoke: Invoke) {
        val data = pendingNotificationData
        if (data == null) {
            invoke.resolve(JSObject())
            return
        }

        pendingNotificationData = null
        invoke.resolve(data)
    }
}
