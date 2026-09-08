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

open class VcpMobileAppControlCommands(activity: Activity) : VcpMobilePermissionCommands(activity) {
    @Command
    fun checkAutoStartPermission(invoke: Invoke) {
        val status = checkAutoStartStatus()
        val result = JSObject()
        result.put("status", status)
        invoke.resolve(result)
    }

    private fun startSettingsActivity(intents: List<Intent>): Boolean {
        for (intent in intents) {
            try {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                activity.startActivity(intent)
                return true
            } catch (error: Exception) {
                Log.d(TAG, "设置入口不可用，继续尝试下一个入口", error)
            }
        }
        return false
    }

    private fun buildAutoStartIntents(manufacturer: String): List<Intent> {
        val intents = mutableListOf<Intent>()
        when {
            manufacturer.contains("xiaomi") || manufacturer.contains("redmi") ->
                intents.add(Intent().setComponent(ComponentName("com.miui.securitycenter", "com.miui.permcenter.autostart.AutoStartManagementActivity")))
            manufacturer.contains("huawei") || manufacturer.contains("honor") -> {
                intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity")))
                intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.optimize.bootstart.BootStartActivity")))
            }
            manufacturer.contains("oppo") || manufacturer.contains("oneplus") || manufacturer.contains("realme") ->
                intents.add(Intent(Settings.ACTION_MANAGE_APPLICATIONS_SETTINGS))
            manufacturer.contains("vivo") -> {
                intents.add(Intent().setComponent(ComponentName("com.iqoo.secure", "com.iqoo.secure.ui.phoneoptimize.BgStartUpManager")))
                intents.add(Intent().setComponent(ComponentName("com.vivo.permissionmanager", "com.vivo.permissionmanager.activity.BgStartUpManagerActivity")))
                intents.add(Intent().setComponent(ComponentName("com.iqoo.secure", "com.iqoo.secure.MainActivity")))
            }
            manufacturer.contains("meizu") -> {
                intents.add(Intent().setComponent(ComponentName("com.meizu.safe", "com.meizu.safe.permission.SmartBGActivity")))
                intents.add(Intent().setComponent(ComponentName("com.meizu.safe", "com.meizu.safe.MainActivity")))
            }
        }
        return intents
    }

    private fun autoStartFallbackIntents(): List<Intent> {
        val appDetails = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
            data = Uri.parse("package:${activity.packageName}")
        }
        return listOf(appDetails, Intent(Settings.ACTION_SETTINGS))
    }

    @Command
    fun requestAutoStartPermission(invoke: Invoke) {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        val success = startSettingsActivity(buildAutoStartIntents(manufacturer)) ||
            startSettingsActivity(autoStartFallbackIntents())
        invoke.resolve(JSObject().apply { put("success", success) })
    }

    private fun buildPowerManagementIntents(manufacturer: String): List<Intent> {
        val intents = mutableListOf<Intent>()
        val packageLabel = activity.applicationInfo.loadLabel(activity.packageManager).toString()
        when {
            manufacturer.contains("xiaomi") || manufacturer.contains("redmi") -> {
                intents.add(Intent("miui.intent.action.OP_POWER_PRIORITY_SETTINGS").apply {
                    putExtra("package_name", activity.packageName)
                    putExtra("package_label", packageLabel)
                })
                intents.add(Intent().setComponent(ComponentName("com.miui.powerkeeper", "com.miui.powerkeeper.ui.HiddenAppsConfigActivity")).apply {
                    putExtra("package_name", activity.packageName)
                    putExtra("package_label", packageLabel)
                })
                intents.add(Intent().setComponent(ComponentName("com.miui.securitycenter", "com.miui.powercenter.PowerSettings")))
            }
            manufacturer.contains("oppo") || manufacturer.contains("oneplus") || manufacturer.contains("realme") -> {
                intents.add(Intent().setComponent(ComponentName("com.coloros.oppoguardelf", "com.coloros.powermanager.fuelgaurd.PowerUsageModelActivity")))
                intents.add(Intent().setComponent(ComponentName("com.coloros.oppoguardelf", "com.coloros.powermanager.fuelgaurd.PowerSavedModeActivity")))
                intents.add(Intent(Intent.ACTION_POWER_USAGE_SUMMARY))
            }
            manufacturer.contains("huawei") || manufacturer.contains("honor") -> {
                intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.power.ui.PowerConsumptionActivity")))
                intents.add(Intent().setComponent(ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.optimize.process.ProtectActivity")))
            }
            manufacturer.contains("vivo") ->
                intents.add(Intent().setComponent(ComponentName("com.iqoo.secure", "com.iqoo.secure.ui.poweroptimize.PowerOptimizeActivity")))
        }
        return intents
    }

    private fun powerManagementFallbackIntents(): List<Intent> {
        val appDetails = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
            data = Uri.parse("package:${activity.packageName}")
        }
        return listOf(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS), appDetails)
    }

    @Command
    fun requestPowerManagementPermission(invoke: Invoke) {
        val manufacturer = Build.MANUFACTURER.lowercase(Locale.ROOT)
        val success = startSettingsActivity(buildPowerManagementIntents(manufacturer)) ||
            startSettingsActivity(powerManagementFallbackIntents())
        invoke.resolve(JSObject().apply { put("success", success) })
    }

    @Command
    fun getFreeDiskSpace(invoke: Invoke) {
        try {
            val path = Environment.getDataDirectory()
            val stat = android.os.StatFs(path.path)
            val blockSize = stat.blockSizeLong
            val availableBlocks = stat.availableBlocksLong
            val totalBlocks = stat.blockCountLong

            val freeBytes = availableBlocks * blockSize
            val totalBytes = totalBlocks * blockSize

            val freeGB = freeBytes.toDouble() / (1024.0 * 1024.0 * 1024.0)
            val totalGB = totalBytes.toDouble() / (1024.0 * 1024.0 * 1024.0)

            val result = JSObject()
            result.put("freeBytes", freeBytes.toDouble())
            result.put("freeGb", freeGB)
            result.put("totalBytes", totalBytes.toDouble())
            result.put("totalGb", totalGB)
            invoke.resolve(result)
        } catch (e: Exception) {
            Log.e(TAG, "获取可用磁盘空间失败", e)
            invoke.reject(e.message ?: "获取可用磁盘空间失败")
        }
    }

    // ==================================================================
    // 权限结果回调
    // ==================================================================
    @PermissionCallback
    fun onPermissionResult(invoke: Invoke) {
        emitPermissionsToWebView()
        invoke.resolve()
    }

    @ActivityCallback
    fun onBatteryOptimizationResult(invoke: Invoke, @Suppress("UNUSED_PARAMETER") result: ActivityResult) {
        emitPermissionsToWebView()
        invoke.resolve()
    }

    @ActivityCallback
    fun onNotificationSettingsResult(invoke: Invoke, @Suppress("UNUSED_PARAMETER") result: ActivityResult) {
        emitPermissionsToWebView()
        invoke.resolve()
    }


    protected override fun emitPermissionsToWebView() {
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

        val json = """{"notification":$notificationGranted,"ring":$ringGranted,"storage":$storageGranted,"microphone":$microphoneGranted,"camera":$cameraGranted,"battery":$batteryOptimizationIgnored,"backgroundRestricted":$backgroundRestricted,"requiresManualPowerManagement":$requiresManualPowerManagement,"overlay":$overlayGranted,"location":$locationGranted}"""
        val script = "window.dispatchEvent(new CustomEvent('vcp-permission-change', { detail: $json }))"
        activity.runOnUiThread {
            webViewRef?.evaluateJavascript(script, null)
        }
    }
}
