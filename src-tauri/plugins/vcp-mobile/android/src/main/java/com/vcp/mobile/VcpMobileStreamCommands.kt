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

open class VcpMobileStreamCommands(activity: Activity) : VcpMobileDeviceCommands(activity) {
    // ==================================================================
    // 前台守护与流式服务
    // ==================================================================
    @Command
    fun acquireForeground(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(AcquireForegroundArgs::class.java)
            com.vcp.mobile.service.ForegroundGuardian.acquire(activity, args.tag, args.priority, args.label, args.screenKeepOn)
            if (args.screenKeepOn) {
                activity.runOnUiThread {
                    activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                }
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "申请前台 lease 失败", e)
            invoke.reject(e.message ?: "未知错误")
        }
    }

    @Command
    fun releaseForeground(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(ReleaseForegroundArgs::class.java)
            com.vcp.mobile.service.ForegroundGuardian.release(activity, args.tag)
            if (!com.vcp.mobile.service.ForegroundGuardian.isScreenKeepOnRequired) {
                activity.runOnUiThread {
                    activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                }
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "释放前台 lease 失败", e)
            invoke.reject(e.message ?: "未知错误")
        }
    }

    @Command
    fun startStreamingService(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(StartStreamArgs::class.java)
            val generation = if (args.agentName.isEmpty()) {
                releaseStreamLease(args)
                null
            } else {
                acquireStreamLease(args)
            }
            invoke.resolve(JSObject().apply {
                generation?.let { put("generation", it) }
            })
        } catch (e: Exception) {
            Log.e(TAG, "启动流式前台服务失败", e)
            invoke.reject(e.message ?: "未知错误")
        }
    }

    protected fun releaseStreamLease(args: StartStreamArgs) {
        if (args.isKeepaliveMode != null) {
            if (args.isKeepaliveMode == true) {
                com.vcp.mobile.service.ForegroundGuardian.acquire(
                    activity,
                    "distributed",
                    com.vcp.mobile.service.ForegroundGuardian.PRIORITY_DISTRIBUTED,
                    "distributed"
                )
            } else {
                com.vcp.mobile.service.ForegroundGuardian.release(activity, "distributed")
            }
            return
        }

        val key = args.streamSessionKeyOrNull()
            ?: throw IllegalArgumentException("流式停止命令缺少完整 ownerType、ownerId、topicId、messageId")
        val tag = com.vcp.mobile.service.ForegroundGuardian.streamTag(key)
        val generation = args.expectedGeneration.requirePositiveGeneration("流式停止")
        com.vcp.mobile.service.ForegroundGuardian.release(activity, tag, generation)
    }

    protected fun acquireStreamLease(args: StartStreamArgs): Long? {
        val key = args.streamSessionKeyOrNull()
            ?: throw IllegalArgumentException("流式启动命令缺少完整 ownerType、ownerId、topicId、messageId")
        val tag = com.vcp.mobile.service.ForegroundGuardian.streamTag(key)
        return when {
            args.agentName.contains("[数据同步]") -> acquireTypedStream(
                tag,
                com.vcp.mobile.service.ForegroundGuardian.PRIORITY_SYNC,
                args.agentName,
                true
            )
            args.agentName.contains("[预渲染重建]") -> acquireTypedStream(
                tag,
                com.vcp.mobile.service.ForegroundGuardian.PRIORITY_PRERENDER,
                args.agentName,
                true
            )
            else -> acquireTypedStream(
                tag,
                com.vcp.mobile.service.ForegroundGuardian.PRIORITY_STREAM,
                args.agentName,
                false
            )
        }
    }

    protected fun acquireTypedStream(tag: String, priority: Int, label: String, screenKeepOn: Boolean): Long? {
        val generation = com.vcp.mobile.service.ForegroundGuardian.acquire(
            activity,
            tag,
            priority,
            label,
            screenKeepOn
        )
        if (screenKeepOn) {
            activity.runOnUiThread {
                activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
        return generation
    }

    @Command
    fun stopStreamingService(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(StopStreamArgs::class.java)
            val sessionKey = args.streamSessionKeyOrNull()
                ?: throw IllegalArgumentException("流式停止命令缺少完整 ownerType、ownerId、topicId、messageId")
            val tag = com.vcp.mobile.service.ForegroundGuardian.streamTag(sessionKey)
            val generation = args.expectedGeneration.requirePositiveGeneration("流式停止")
            com.vcp.mobile.service.ForegroundGuardian.release(activity, tag, generation)
            activity.runOnUiThread {
                if (!com.vcp.mobile.service.ForegroundGuardian.isScreenKeepOnRequired) {
                    activity.window.clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                }
            }
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "停止流式前台服务失败", e)
            invoke.reject(e.message ?: "未知错误")
        }
    }

    @Command
    fun acquireWakeLock(invoke: Invoke) {
        try {
            com.vcp.mobile.service.ForegroundGuardian.acquire(
                activity, "manual_keepalive",
                com.vcp.mobile.service.ForegroundGuardian.PRIORITY_DISTRIBUTED,
                "[后台保活]"
            )
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "申请手动保活锁失败", e)
            invoke.reject(e.message ?: "未知错误")
        }
    }

    @Command
    fun releaseWakeLock(invoke: Invoke) {
        try {
            com.vcp.mobile.service.ForegroundGuardian.release(activity, "manual_keepalive")
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "释放手动保活锁失败", e)
            invoke.reject(e.message ?: "未知错误")
        }
    }

    /** Rust bootstrap 用于收口 BootReceiver 遗留的分布式恢复状态。 */
    @Command
    fun releaseDistributedForeground(invoke: Invoke) {
        try {
            com.vcp.mobile.service.ForegroundGuardian.releaseDistributed(activity)
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "清理分布式前台恢复状态失败", e)
            invoke.reject(e.message ?: "清理分布式前台恢复状态失败")
        }
    }

    @Command
    fun startSensorCollection(invoke: Invoke) {
        try {
            activity.runOnUiThread {
                sensorStatusManager.start()
                invoke.resolve()
            }
        } catch (e: Exception) {
            Log.e(TAG, "启动传感器采集失败", e)
            invoke.reject(e.message ?: "启动传感器采集失败")
        }
    }

    @Command
    fun stopSensorCollection(invoke: Invoke) {
        try {
            activity.runOnUiThread {
                sensorStatusManager.stop()
                invoke.resolve()
            }
        } catch (e: Exception) {
            Log.e(TAG, "停止传感器采集失败", e)
            invoke.reject(e.message ?: "停止传感器采集失败")
        }
    }

    @Command
    fun getSensorData(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(GetSensorDataArgs::class.java)
            val result = sensorStatusManager.getSensorData(args.type)
            invoke.resolve(result)
        } catch (e: Exception) {
            Log.e(TAG, "获取传感器数据失败", e)
            invoke.reject(e.message ?: "获取传感器数据失败")
        }
    }
}
