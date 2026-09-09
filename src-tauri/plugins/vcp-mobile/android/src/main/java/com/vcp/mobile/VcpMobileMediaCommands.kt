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

open class VcpMobileMediaCommands(activity: Activity) : VcpMobileGalleryCommands(activity) {
    // ==================================================================
    // WebView 高性能截图
    // ==================================================================
    @Command
    fun captureWindowSnapshot(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(CaptureWindowSnapshotArgs::class.java)
        } catch (_: Throwable) {
            CaptureWindowSnapshotArgs()
        }

        val maxWidth = args.maxWidth.coerceIn(160, 420)
        val quality = args.quality.coerceIn(45, 85)

        // 💥 去掉锁机制，采用完全异步的 resolve/reject 调用模式，避免 Tokio 核心线程被 latch.await 挂起
        activity.runOnUiThread {
            try {
                val rootView = activity.window.decorView.rootView
                val sourceWidth = rootView.width
                val sourceHeight = rootView.height
                if (sourceWidth <= 0 || sourceHeight <= 0) {
                    invoke.reject("视图尺寸无效：${sourceWidth}x${sourceHeight}")
                    return@runOnUiThread
                }

                val scale = min(1f, maxWidth.toFloat() / sourceWidth.toFloat())
                val outputWidth = max(1, (sourceWidth * scale).roundToInt())
                val outputHeight = max(1, (sourceHeight * scale).roundToInt())
                val snapshot = Bitmap.createBitmap(outputWidth, outputHeight, Bitmap.Config.RGB_565)
                val canvas = Canvas(snapshot)
                canvas.scale(scale, scale)
                rootView.draw(canvas)

                val encoded = ByteArrayOutputStream()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    snapshot.compress(Bitmap.CompressFormat.WEBP_LOSSY, quality, encoded)
                } else {
                    @Suppress("DEPRECATION")
                    snapshot.compress(Bitmap.CompressFormat.WEBP, quality, encoded)
                }
                snapshot.recycle() // 及时物理释放内存，防御 WebView 渲染高频截图导致 OOM

                val base64 = Base64.encodeToString(encoded.toByteArray(), Base64.NO_WRAP)
                val resultObject = JSObject().apply {
                    put("dataUrl", "data:image/webp;base64,$base64")
                    put("width", outputWidth)
                    put("height", outputHeight)
                }
                invoke.resolve(resultObject)
            } catch (e: Throwable) {
                Log.e(TAG, "窗口截图失败", e)
                invoke.reject(e.message ?: "窗口截图失败")
            }
        }
    }

    @Command
    fun processImage(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessImageArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("参数无效：${e.message}")
            return
        }

        MediaBridge.processImageAsync(args.path, activity) { result ->
            result.onSuccess { outputPath ->
                val resObj = JSObject().apply {
                    put("path", outputPath)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "处理图片失败")
            }
        }
    }

    @Command
    fun processVideo(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessVideoArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("参数无效：${e.message}")
            return
        }

        MediaBridge.processVideoAsync(args.path, activity) { result ->
            result.onSuccess { framePaths ->
                val arr = JSArray()
                for (p in framePaths) {
                    arr.put(p)
                }
                val resObj = JSObject().apply {
                    put("paths", arr)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "处理视频失败")
            }
        }
    }

    @Command
    fun processAudio(invoke: Invoke) {
        val args = try {
            invoke.parseArgs(ProcessAudioArgs::class.java)
        } catch (e: Throwable) {
            invoke.reject("参数无效：${e.message}")
            return
        }

        MediaBridge.processAudioAsync(args.path, activity) { result ->
            result.onSuccess { outputPath ->
                val resObj = JSObject().apply {
                    put("path", outputPath)
                }
                invoke.resolve(resObj)
            }.onFailure { exception ->
                invoke.reject(exception.message ?: "处理音频失败")
            }
        }
    }
}
