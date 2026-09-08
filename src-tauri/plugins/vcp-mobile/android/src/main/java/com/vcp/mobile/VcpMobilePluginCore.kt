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
import android.media.RingtoneManager
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

open class VcpMobilePluginCore(protected val activity: Activity) : Plugin(activity) {
    protected val TAG = "VcpMobilePlugin"

    protected val activityLifecycleCallbacks = object : android.app.Application.ActivityLifecycleCallbacks {
        override fun onActivityResumed(a: Activity) {
            if (a === activity) {
                isAppInForeground = true
                com.vcp.mobile.service.ForegroundGuardian.onAppForegroundChanged(
                    activity.applicationContext,
                    true
                )

                if (com.vcp.mobile.service.ForegroundGuardian.isScreenKeepOnRequired) {
                    activity.runOnUiThread {
                        activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    }
                }
            }
        }
        override fun onActivityPaused(a: Activity) {
            if (a === activity) {
                isAppInForeground = false
                com.vcp.mobile.service.ForegroundGuardian.onAppForegroundChanged(
                    activity.applicationContext,
                    false
                )

                if (com.vcp.mobile.service.ForegroundGuardian.isScreenKeepOnRequired) {
                    activity.runOnUiThread {
                        activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    }
                }
            }
        }
        override fun onActivityCreated(a: Activity, savedInstanceState: android.os.Bundle?) {}
        override fun onActivityStarted(a: Activity) {}
        override fun onActivityStopped(a: Activity) {}
        override fun onActivitySaveInstanceState(a: Activity, outState: android.os.Bundle) {}
        override fun onActivityDestroyed(a: Activity) {}
    }

    init {
        com.vcp.mobile.service.ForegroundGuardian.onAppForegroundChanged(
            activity.applicationContext,
            true
        )
        activity.application.registerActivityLifecycleCallbacks(activityLifecycleCallbacks)
    }

    val pluginActivity: Activity get() = activity
    var webViewRef: WebView? = null
    protected var isAppInForeground = true
    protected var pendingNotificationData: JSObject? = null
    protected var autoStartReflectionFailureLogged = false

    protected fun handleNotificationIntent(intent: Intent) {
        val payload = NotificationPayloadBridge.fromIntent(intent) ?: return
        Log.i(
            TAG,
            "[handleNotificationIntent] 收到通知点击: topicId=${payload.topicId}, " +
                "ownerType=${payload.ownerType}, ownerId=${payload.ownerId}, requestId=${payload.requestId}"
        )
        val data = NotificationPayloadBridge.toJsObject(payload)
        pendingNotificationData = data

        val webView = webViewRef
        if (webView != null) {
            val dataJson = data.toString()
            val safeJson = escapeJsonForJsString(dataJson)
            val script = "window.dispatchEvent(new CustomEvent('vcp-notification-click', { detail: JSON.parse(\"$safeJson\") }))"
            activity.runOnUiThread {
                webView.evaluateJavascript(script, null)
            }
        } else {
            Log.w(TAG, "[handleNotificationIntent] WebView 尚未就绪，已缓存通知数据")
        }
        NotificationPayloadBridge.consume(intent)
    }
    protected val keyboardInsetsManager = KeyboardInsetsManager(activity)
    protected val lifecycleBridge = LifecycleBridge(
        onResumeHook = {
            emitPermissionsToWebView()
            keyboardInsetsManager.requestInsetsRefresh()
        },
        onConfigurationChangedHook = {
            keyboardInsetsManager.requestInsetsRefresh()
        }
    )
    protected val batteryStatusManager = BatteryStatusManager(activity)
    protected val networkStatusManager = NetworkStatusManager(activity)
    protected val cpuStatusManager = CpuStatusManager(activity)
    protected val gpuStatusManager = GpuStatusManager(activity)
    protected val floatingWindowManager by lazy { FloatingWindowManager(activity) }
    protected val sensorStatusManager = SensorStatusManager(activity)
    protected val shareIntentHandler = ShareIntentHandler(this)
    protected val fileIoExecutor = java.util.concurrent.Executors.newSingleThreadExecutor()
    protected val oomGuardExecutor = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()
    protected val oomGuardStarted = java.util.concurrent.atomic.AtomicBoolean(false)
    protected var cameraTempFile: java.io.File? = null
    protected var networkCallback: android.net.ConnectivityManager.NetworkCallback? = null
    protected var lastConnected: Boolean? = null
    protected var isNetworkMonitoringStarted = false

    protected fun startHelperServiceInternal() {
        val intent = Intent(activity, com.vcp.mobile.service.SseProxyService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            activity.startForegroundService(intent)
        } else {
            activity.startService(intent)
        }
        Log.i(TAG, "已请求启动 SseProxyService")
    }

    protected val AGENT_MESSAGE_CHANNEL_ID = "agent_message_alerts_v2"
    protected val AGENT_MESSAGE_NOTIF_BASE_ID = 0x41474D00
    protected val AGENT_MESSAGE_NOTIF_SEQUENCE_MASK = 0x3FFFFFFF
    protected fun createAgentMessageNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val name = "Agent 消息提醒"
            val descriptionText = "显示分布式 AgentMessage 插件推送的消息"
            val agentMessageSoundUri = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_NOTIFICATION)
            val soundAttributes = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_NOTIFICATION)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build()
            val importance = android.app.NotificationManager.IMPORTANCE_HIGH
            val channel = android.app.NotificationChannel(AGENT_MESSAGE_CHANNEL_ID, name, importance).apply {
                description = descriptionText
                enableVibration(true)
                vibrationPattern = longArrayOf(0, 180, 80, 180)
                setSound(agentMessageSoundUri, soundAttributes)
            }
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            notificationManager.createNotificationChannel(channel)
            Log.i(TAG, "AgentMessage 通知频道已就绪：id=$AGENT_MESSAGE_CHANNEL_ID importance=$importance")
        }
    }
    protected fun generateNativeThumbnail(context: Context, originalFile: java.io.File, hash: String): String? {
        val uploadsDir = java.io.File(context.cacheDir, "uploads").apply { mkdirs() }
        val thumbDir = java.io.File(uploadsDir, "thumbnails").apply { mkdirs() }
        val thumbFile = java.io.File(thumbDir, "${hash}_thumb.webp")
        if (thumbFile.exists()) return thumbFile.absolutePath

        try {
            val bitmap = createThumbnailBitmap(originalFile) ?: return null
            writeThumbnail(bitmap, thumbFile)
            bitmap.recycle() // 显式释放 Native 物理内存，防范溢出
            return thumbFile.absolutePath
        } catch (e: Exception) {
            Log.e(TAG, "生成原生缩略图失败", e)
            return null
        }
    }

    private fun createThumbnailBitmap(originalFile: java.io.File): Bitmap? {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            return android.media.ThumbnailUtils.createImageThumbnail(
                originalFile,
                android.util.Size(200, 200),
                null
            )
        }
        val options = android.graphics.BitmapFactory.Options().apply { inJustDecodeBounds = true }
        android.graphics.BitmapFactory.decodeFile(originalFile.absolutePath, options)
        val sampleSize = calculateSampleSize(options.outWidth, options.outHeight)
        options.inJustDecodeBounds = false
        options.inSampleSize = sampleSize
        val rawBitmap = android.graphics.BitmapFactory.decodeFile(originalFile.absolutePath, options) ?: return null
        return scaleThumbnail(rawBitmap)
    }

    private fun calculateSampleSize(width: Int, height: Int): Int {
        var sampleSize = 1
        if (width > 200 || height > 200) {
            val halfHeight = height / 2
            val halfWidth = width / 2
            while (halfHeight / sampleSize >= 200 && halfWidth / sampleSize >= 200) {
                sampleSize *= 2
            }
        }
        return sampleSize
    }

    private fun scaleThumbnail(rawBitmap: Bitmap): Bitmap {
        val (newWidth, newHeight) = if (rawBitmap.width >= rawBitmap.height) {
            val ratio = rawBitmap.width.toFloat() / rawBitmap.height.toFloat()
            ((200f * ratio).toInt() to 200)
        } else {
            val ratio = rawBitmap.height.toFloat() / rawBitmap.width.toFloat()
            (200 to (200f * ratio).toInt())
        }
        val scaled = Bitmap.createScaledBitmap(rawBitmap, newWidth, newHeight, true)
        if (scaled != rawBitmap) rawBitmap.recycle()
        return scaled
    }

    private fun writeThumbnail(bitmap: Bitmap, target: java.io.File) {
        java.io.FileOutputStream(target).use { out ->
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                bitmap.compress(Bitmap.CompressFormat.WEBP_LOSSY, 80, out)
            } else {
                @Suppress("DEPRECATION")
                bitmap.compress(Bitmap.CompressFormat.WEBP, 80, out)
            }
        }
    }

    protected fun escapeJsonForJsString(json: String): String {
        return json
            .replace("\\", "\\\\")
            .replace("\"", "\\\"")
            .replace("\'", "\\\'")
            .replace("\n", "\\n")
            .replace("\r", "\\r")
    }
    protected fun resolveSafeLocalFile(context: Context, path: String): java.io.File? {
        val allowedDirectories = listOfNotNull(
            context.cacheDir,
            context.filesDir,
            context.getExternalFilesDir(null),
            context.externalCacheDir,
        ).mapNotNull { directory ->
            try {
                directory.canonicalFile
            } catch (_: Exception) {
                null
            }
        }
        return resolveCanonicalFileInDirectories(java.io.File(path), allowedDirectories)
    }

    protected fun isSafeLocalPath(context: Context, path: String): Boolean {
        return resolveSafeLocalFile(context, path) != null
    }

    protected open fun emitPermissionsToWebView() = Unit

}
