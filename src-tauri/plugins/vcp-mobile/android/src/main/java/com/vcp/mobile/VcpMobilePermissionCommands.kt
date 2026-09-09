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

open class VcpMobilePermissionCommands(activity: Activity) : VcpMobilePluginCore(activity) {
    // ==================================================================
    // 权限与应用控制
    // ==================================================================
    protected fun hasNotificationPermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }
    }

    protected fun hasStoragePermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= 34) {
            val hasAll = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
            val hasVisualSelected = ContextCompat.checkSelfPermission(activity, "android.permission.READ_MEDIA_VISUAL_USER_SELECTED") == PackageManager.PERMISSION_GRANTED
            hasAll || hasVisualSelected
        } else if (Build.VERSION.SDK_INT >= 33) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_MEDIA_IMAGES) == PackageManager.PERMISSION_GRANTED
        } else if (Build.VERSION.SDK_INT >= 29) {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        } else {
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED &&
                ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
        }
    }

    protected fun isBackgroundExecutionRestricted(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) return false
        val activityManager = activity.getSystemService(Context.ACTIVITY_SERVICE) as? android.app.ActivityManager
        return activityManager?.isBackgroundRestricted ?: false
    }

    protected fun requiresManualPowerManagementConfirmation(): Boolean {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        return manufacturer.contains("xiaomi") ||
            manufacturer.contains("redmi") ||
            manufacturer.contains("oppo") ||
            manufacturer.contains("oneplus") ||
            manufacturer.contains("realme") ||
            manufacturer.contains("huawei") ||
            manufacturer.contains("honor") ||
            manufacturer.contains("vivo") ||
            manufacturer.contains("meizu")
    }

    protected fun hasAgentMessageRingCapability(notificationGranted: Boolean): Boolean {
        if (!notificationGranted) return false
        if (!androidx.core.app.NotificationManagerCompat.from(activity).areNotificationsEnabled()) return false

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            createAgentMessageNotificationChannel()
            val notificationManager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
            val channel = notificationManager.getNotificationChannel(AGENT_MESSAGE_CHANNEL_ID) ?: return false
            val highEnough = channel.importance >= android.app.NotificationManager.IMPORTANCE_DEFAULT
            val hasSound = channel.sound != null
            val hasVibration = channel.shouldVibrate()
            Log.i(
                TAG,
                "AgentMessage 铃声能力：importance=${channel.importance}，sound=$hasSound，vibration=$hasVibration"
            )
            return highEnough && (hasSound || hasVibration)
        }

        return true
    }

    protected fun openAgentMessageNotificationSettings(invoke: Invoke) {
        try {
            createAgentMessageNotificationChannel()
            val intent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).apply {
                    putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
                }
            } else {
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                    data = Uri.parse("package:${activity.packageName}")
                }
            }
            startActivityForResult(invoke, intent, "onNotificationSettingsResult")
        } catch (e: Exception) {
            // 如果频道级设置打不开（某些定制 ROM），降级到应用级通知设置
            try {
                val appLevel = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).apply {
                    putExtra(Settings.EXTRA_APP_PACKAGE, activity.packageName)
                }
                startActivityForResult(invoke, appLevel, "onNotificationSettingsResult")
            } catch (e2: Exception) {
                // 最终兜底：应用详情页
                val fallback = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                    data = Uri.parse("package:${activity.packageName}")
                }
                startActivityForResult(invoke, fallback, "onNotificationSettingsResult")
            }
        }
    }

    @Command
    fun checkAllPermissions(invoke: Invoke) {
        val pm = activity.getSystemService(Context.POWER_SERVICE) as PowerManager

        val notificationGranted = hasNotificationPermission()
        val ringGranted = hasAgentMessageRingCapability(notificationGranted)
        val storageGranted = hasStoragePermission()

        val microphoneGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED
        val cameraGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
        val locationGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED ||
            ContextCompat.checkSelfPermission(activity, android.Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED

        val batteryOptimizationIgnored = pm.isIgnoringBatteryOptimizations(activity.packageName)
        val backgroundRestricted = isBackgroundExecutionRestricted()
        val requiresManualPowerManagement = requiresManualPowerManagementConfirmation()
        val overlayGranted = floatingWindowManager.hasOverlayPermission()

        val result = JSObject()
        result.put("notification", notificationGranted)
        result.put("ring", ringGranted)
        result.put("storage", storageGranted)
        result.put("microphone", microphoneGranted)
        result.put("camera", cameraGranted)
        result.put("location", locationGranted)
        result.put("battery", batteryOptimizationIgnored)
        result.put("backgroundRestricted", backgroundRestricted)
        result.put("requiresManualPowerManagement", requiresManualPowerManagement)
        result.put("overlay", overlayGranted)

        invoke.resolve(result)
    }

    @Command
    fun requestAndroidPermission(invoke: Invoke) {
        val args = invoke.parseArgs(RequestPermissionArgs::class.java)
        when (args.type) {
            "notification" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    requestPermissionForAlias("notification", invoke, "onPermissionResult")
                } else {
                    emitPermissionsToWebView()
                    invoke.resolve()
                }
            }
            "ring" -> {
                if (Build.VERSION.SDK_INT >= 33 && !hasNotificationPermission()) {
                    requestPermissionForAlias("notification", invoke, "onPermissionResult")
                } else {
                    openAgentMessageNotificationSettings(invoke)
                }
            }
            "storage" -> {
                if (Build.VERSION.SDK_INT >= 33) {
                    requestPermissionForAlias("storage", invoke, "onPermissionResult")
                } else {
                    requestPermissionForAlias("storageLegacy", invoke, "onPermissionResult")
                }
            }
            "microphone" -> {
                requestPermissionForAlias("microphone", invoke, "onPermissionResult")
            }
            "camera" -> {
                requestPermissionForAlias("camera", invoke, "onPermissionResult")
            }
            "location" -> {
                requestPermissionForAlias("location", invoke, "onPermissionResult")
            }
            "battery" -> {
                try {
                    val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                        data = Uri.parse("package:${activity.packageName}")
                    }
                    startActivityForResult(invoke, intent, "onBatteryOptimizationResult")
                } catch (e: Exception) {
                    val intent = Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                    startActivityForResult(invoke, intent, "onBatteryOptimizationResult")
                }
            }
        }
    }

    @Command
    fun moveTaskToBack(invoke: Invoke) {
        activity.moveTaskToBack(true)
        invoke.resolve()
    }

    @Command
    fun check_notification_listener_permission(invoke: Invoke) {
        val context = activity.applicationContext
        val pkgName = context.packageName
        val flat = Settings.Secure.getString(context.contentResolver, "enabled_notification_listeners")
        var isEnabled = false
        if (!flat.isNullOrEmpty()) {
            val names = flat.split(":")
            for (name in names) {
                val cn = ComponentName.unflattenFromString(name)
                if (cn != null && cn.packageName == pkgName) {
                    isEnabled = true
                    break
                }
            }
        }
        val ret = JSObject()
        ret.put("enabled", isEnabled)
        invoke.resolve(ret)
    }

    @Command
    fun request_notification_listener_permission(invoke: Invoke) {
        try {
            val intent = Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            activity.startActivity(intent)
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject("打开通知监听设置失败：${e.message}")
        }
    }

    protected fun startOomScoreGuard() {
        if (!oomGuardStarted.compareAndSet(false, true)) return
        oomGuardExecutor.execute {
            try {
                // 利用 topjohnwu 的 superuser 库检查 root 状态
                if (Shell.getShell().isRoot) {
                    val pid = android.os.Process.myPid()
                    Log.i(TAG, "OomScoreGuard：已检测到 Root，正在将 PID $pid 的 OOM 分数调整为 -900")
                    val applyOomScore = Runnable {
                        try {
                            // 强行把 oom_score_adj 改为 -900
                            Shell.cmd("echo -900 > /proc/$pid/oom_score_adj").exec()
                        } catch (e: Exception) {
                            Log.e(TAG, "OomScoreGuard：写入命令失败", e)
                        }
                    }
                    applyOomScore.run()
                    oomGuardExecutor.scheduleWithFixedDelay(
                        applyOomScore,
                        20,
                        20,
                        TimeUnit.SECONDS
                    )
                } else {
                    Log.i(TAG, "OomScoreGuard：设备未获取 Root，跳过 OOM 分数锁定")
                }
            } catch (e: Exception) {
                Log.e(TAG, "OomScoreGuard 执行失败", e)
            }
        }
    }

    protected fun checkAutoStartStatus(): String {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        if (manufacturer.contains("xiaomi") || manufacturer.contains("redmi")) {
            val ops = activity.getSystemService(Context.APP_OPS_SERVICE) as? android.app.AppOpsManager
            if (ops != null) {
                try {
                    val method = ops.javaClass.getMethod(
                        "checkOpNoThrow",
                        Int::class.javaPrimitiveType,
                        Int::class.javaPrimitiveType,
                        String::class.java
                    )
                    // 10008 是 MIUI / HyperOS AppOpsManager 中的 OP_AUTO_START。
                    val mode = method.invoke(
                        ops,
                        10008,
                        activity.applicationInfo.uid,
                        activity.packageName
                    ) as Int
                    autoStartReflectionFailureLogged = false
                    return if (mode == android.app.AppOpsManager.MODE_ALLOWED) "true" else "false"
                } catch (e: Exception) {
                    if (!autoStartReflectionFailureLogged) {
                        autoStartReflectionFailureLogged = true
                        Log.w(TAG, "checkAutoStartStatus：反射检查失败，将回退为手动确认", e)
                    }
                }
            }
        }
        return "unsupported"
    }
}
