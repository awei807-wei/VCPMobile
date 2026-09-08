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

open class VcpMobileDeviceCommands(activity: Activity) : VcpMobileAppControlCommands(activity) {
    @Command
    fun requestOverlayPermission(invoke: Invoke) {
        floatingWindowManager.requestOverlayPermission()
        invoke.resolve()
    }

    @Command
    fun toggleFloatingBall(invoke: Invoke) {
        val args = invoke.parseArgs(ToggleFloatingBallArgs::class.java)
        val success = floatingWindowManager.toggleFloatingBall(args.show)
        val result = JSObject()
        result.put("success", success)
        invoke.resolve(result)
    }

    // ==================================================================
    // 屏幕与设备状态
    // ==================================================================
    @Command
    fun setKeepScreenOn(invoke: Invoke) {
        activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        invoke.resolve()
    }

    @Command
    fun clearKeepScreenOn(invoke: Invoke) {
        activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        invoke.resolve()
    }

    @Command
    fun getBatteryStatus(invoke: Invoke) {
        try {
            val status = batteryStatusManager.getStatusJson()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "获取电池状态失败", e)
            invoke.reject(e.message ?: "获取电池状态失败")
        }
    }

    @Command
    fun getNetworkStatus(invoke: Invoke) {
        try {
            val status = networkStatusManager.getNetworkStatus()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "获取网络状态失败", e)
            invoke.reject(e.message ?: "获取网络状态失败")
        }
    }

    @Command
    fun getCpuThermalStatus(invoke: Invoke) {
        try {
            val status = cpuStatusManager.getThermalStatus()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "获取 CPU 温度状态失败", e)
            invoke.reject(e.message ?: "获取 CPU 温度状态失败")
        }
    }

    @Command
    fun getGpuStatus(invoke: Invoke) {
        try {
            val status = gpuStatusManager.getGpuStatusJson()
            invoke.resolve(status)
        } catch (e: Exception) {
            Log.e(TAG, "获取 GPU 状态失败", e)
            invoke.reject(e.message ?: "获取 GPU 状态失败")
        }
    }

    @Command
    fun checkRootAccess(invoke: Invoke) {
        fileIoExecutor.execute {
            try {
                val isRoot = Shell.getShell().isRoot
                val result = JSObject()
                result.put("isRoot", isRoot)
                invoke.resolve(result)
            } catch (e: Exception) {
                val result = JSObject()
                result.put("isRoot", false)
                invoke.resolve(result)
            }
        }
    }

    @Command
    fun writeClipboard(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(WriteClipboardArgs::class.java)
            activity.runOnUiThread {
                try {
                    val clipboard = activity.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                    val clip = android.content.ClipData.newPlainText("VCP 分布式复制", args.content)
                    clipboard.setPrimaryClip(clip)
                    invoke.resolve()
                } catch (e: Exception) {
                    invoke.reject(e.message ?: "在 UI 线程写入剪贴板失败")
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "解析参数失败")
        }
    }

    @Command
    fun readClipboard(invoke: Invoke) {
        try {
            activity.runOnUiThread {
                try {
                    val clipboard = activity.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager
                    val clipData = clipboard.primaryClip
                    val content = if (clipData != null && clipData.itemCount > 0) {
                        clipData.getItemAt(0).text?.toString() ?: ""
                    } else {
                        ""
                    }
                    val result = JSObject().apply {
                        put("content", content)
                    }
                    invoke.resolve(result)
                } catch (e: Exception) {
                    invoke.reject(e.message ?: "在 UI 线程读取剪贴板失败")
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "执行 readClipboard 失败")
        }
    }

    @Command
    fun sendLocalNotification(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(SendLocalNotificationArgs::class.java)
            val context = activity.applicationContext
            val notificationManager = context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager

            val channelId = "vcp_distributed_alert"
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
                val channel = android.app.NotificationChannel(
                    channelId,
                    "VCP 分布式节点提醒",
                    android.app.NotificationManager.IMPORTANCE_HIGH
                )
                notificationManager.createNotificationChannel(channel)
            }

            val notification = androidx.core.app.NotificationCompat.Builder(context, channelId)
                .setContentTitle(args.title)
                .setContentText(args.body)
                .setSmallIcon(R.drawable.ic_vcp_notification)
                .setPriority(androidx.core.app.NotificationCompat.PRIORITY_HIGH)
                .setAutoCancel(true)
                .build()

            notificationManager.notify((System.currentTimeMillis() % 100000).toInt(), notification)
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject(e.message ?: "发送通知失败")
        }
    }

    @Command
    fun runRootCommand(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(RunRootCommandArgs::class.java)
            fileIoExecutor.execute {
                try {
                    val output = Shell.cmd(args.command).exec().out
                    val result = JSObject().apply {
                        put("success", true)
                        put("output", output.joinToString("\n"))
                    }
                    invoke.resolve(result)
                } catch (e: Exception) {
                    val result = JSObject().apply {
                        put("success", false)
                        put("output", e.message ?: "Shell 执行失败")
                    }
                    invoke.resolve(result)
                }
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "参数解析失败")
        }
    }

    @Command
    fun launchRootManager(invoke: Invoke) {
        try {
            val managers = listOf(
                "com.topjohnwu.magisk" to "Magisk",
                "me.weishu.kernelsu" to "KernelSU",
                "me.tool.apatch" to "APatch"
            )
            var launched = false
            for ((pkg, name) in managers) {
                try {
                    val intent = activity.packageManager.getLaunchIntentForPackage(pkg)
                    if (intent != null) {
                        intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
                        activity.startActivity(intent)
                        launched = true
                        val result = JSObject().apply {
                            put("success", true)
                            put("manager", name)
                        }
                        invoke.resolve(result)
                        break
                    }
                } catch (e: Exception) {
                    // 继续检查下一个软件包
                }
            }
            if (!launched) {
                val result = JSObject().apply {
                    put("success", false)
                    put("message", "未找到支持的 Root 管理器 (Magisk, KernelSU, APatch)。")
                }
                invoke.resolve(result)
            }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "启动 Root 管理器失败")
        }
    }
}
