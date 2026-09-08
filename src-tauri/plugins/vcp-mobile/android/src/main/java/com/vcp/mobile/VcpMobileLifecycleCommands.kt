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

open class VcpMobileLifecycleCommands(activity: Activity) : VcpMobileStreamCommands(activity) {
    // ==================================================================
    // 插件生命周期
    // ==================================================================

    protected fun emitNetworkStatusToWebView() {
        val status = networkStatusManager.getNetworkStatus()
        val connected = status.optBoolean("connected", false)
        if (connected != lastConnected) {
            lastConnected = connected
            trigger("vcp-network-status-changed", status)
        }
    }

    @Command
    fun startNetworkMonitoring(invoke: Invoke) {
        if (isNetworkMonitoringStarted) {
            invoke.resolve()
            return
        }
        try {
            val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
            val request = android.net.NetworkRequest.Builder()
                .addCapability(android.net.NetworkCapabilities.NET_CAPABILITY_INTERNET)
                .build()
            networkCallback = object : android.net.ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: android.net.Network) {
                    emitNetworkStatusToWebView()
                }
                override fun onLost(network: android.net.Network) {
                    emitNetworkStatusToWebView()
                }
                override fun onCapabilitiesChanged(network: android.net.Network, networkCapabilities: android.net.NetworkCapabilities) {
                    emitNetworkStatusToWebView()
                }
            }
            cm.registerNetworkCallback(request, networkCallback!!)
            isNetworkMonitoringStarted = true
            Log.i(TAG, "[Network] 原生网络状态监控已启动")
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "注册网络回调失败", e)
            invoke.reject(e.message ?: "注册网络回调失败")
        }
    }

    override fun load(webView: WebView) {
        super.load(webView)
        webViewRef = webView

        keyboardInsetsManager.attach(webView)
        lifecycleBridge.attach(activity, this)

        startOomScoreGuard()

        // 冷启动：处理传递给 Activity 的初始 intent
        shareIntentHandler.handleShareIntent(activity.intent)
        shareIntentHandler.injectShareData(webView)
        handleNotificationIntent(activity.intent)
    }

    override fun onDestroy(activity: AppCompatActivity) {
        activity.application.unregisterActivityLifecycleCallbacks(activityLifecycleCallbacks)
        webViewRef = null
        lifecycleBridge.detach()
        try {
            if (networkCallback != null) {
                val cm = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as android.net.ConnectivityManager
                cm.unregisterNetworkCallback(networkCallback!!)
                networkCallback = null
                isNetworkMonitoringStarted = false
            }
        } catch (_: Exception) {}
        try {
            // 锁由 ForegroundGuardian 统一管理
        } catch (_: Exception) {}
        try {
            fileIoExecutor.shutdown()
        } catch (_: Exception) {}
        try {
            oomGuardExecutor.shutdownNow()
        } catch (_: Exception) {}
        super.onDestroy(activity)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        lifecycleBridge.onConfigurationChanged(newConfig)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        shareIntentHandler.handleShareIntent(intent)
        handleNotificationIntent(intent)
    }
}
