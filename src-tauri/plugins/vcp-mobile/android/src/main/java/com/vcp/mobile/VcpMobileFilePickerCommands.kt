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

open class VcpMobileFilePickerCommands(activity: Activity) : VcpMobileLifecycleCommands(activity) {
    // ==================================================================
    // 分区存储文件选择器与原生缩略图生成（方案 B）
    // ==================================================================
    @PermissionCallback
    fun onCameraPermissionResult(invoke: Invoke) {
        if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            launchCameraIntent(invoke)
        } else {
            Log.w(TAG, "[onCameraPermissionResult] 相机权限被拒绝")
            invoke.reject("相机权限被拒绝")
        }
    }

    protected fun launchCameraIntent(invoke: Invoke) {
        try {
            val uploadsDir = java.io.File(activity.cacheDir, "uploads").apply { mkdirs() }
            val tempFile = java.io.File(uploadsDir, "camera_${System.currentTimeMillis()}.jpg")
            cameraTempFile = tempFile

            val authority = "${activity.packageName}.fileprovider"
            val uri = try {
                FileProvider.getUriForFile(activity, authority, tempFile)
            } catch (e: Exception) {
                FileProvider.getUriForFile(activity, "${activity.packageName}.opener.fileprovider", tempFile)
            }

            val intent = Intent(android.provider.MediaStore.ACTION_IMAGE_CAPTURE).apply {
                putExtra(android.provider.MediaStore.EXTRA_OUTPUT, uri)
                addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
            }
            startActivityForResult(invoke, intent, "onCameraResult")
        } catch (e: Throwable) {
            Log.e(TAG, "[launchCameraIntent] 启动相机意图失败", e)
            invoke.reject("启动相机失败：${e.message}")
        }
    }

    @Command
    fun pickFile(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(PickFileArgs::class.java)
            val mode = args.mode
            Log.i(TAG, "[pickFile] 已调用，模式：$mode")

            when (mode) {
                "camera" -> {
                    if (ContextCompat.checkSelfPermission(activity, android.Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
                        requestPermissionForAlias("camera", invoke, "onCameraPermissionResult")
                        return
                    }
                    launchCameraIntent(invoke)
                }
                "gallery" -> {
                    val intent = Intent(Intent.ACTION_GET_CONTENT).apply {
                        type = "image/*"
                        addCategory(Intent.CATEGORY_OPENABLE)
                    }
                    startActivityForResult(invoke, intent, "onPickFileResult")
                }
                else -> {
                    val intent = Intent(Intent.ACTION_GET_CONTENT).apply {
                        type = "*/*"
                        addCategory(Intent.CATEGORY_OPENABLE)
                    }
                    startActivityForResult(invoke, intent, "onPickFileResult")
                }
            }
        } catch (e: Throwable) {
            Log.e(TAG, "[pickFile] 启动结果 Activity 失败", e)
            invoke.reject("启动原生文件选择器失败：${e.message}")
        }
    }

    @ActivityCallback
    fun onCameraResult(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode != Activity.RESULT_OK) {
            Log.w(TAG, "[onCameraResult] 相机拍摄已取消或失败")
            cameraTempFile?.delete()
            cameraTempFile = null
            invoke.reject("已取消")
            return
        }

        val photoFile = cameraTempFile
        if (photoFile == null || !photoFile.exists()) {
            Log.e(TAG, "[onCameraResult] 临时照片不存在")
            invoke.reject("拍摄失败：找不到临时文件")
            return
        }
        cameraTempFile = null
        fileIoExecutor.execute { processCapturedPhoto(photoFile, invoke) }
    }

    private fun processCapturedPhoto(photoFile: java.io.File, invoke: Invoke) {
        try {
            val originalName = "Camera_${System.currentTimeMillis()}.jpg"
            val mimeType = "image/jpeg"
            val size = photoFile.length()
            publishFileStart(originalName, size, mimeType)
            val hash = hashFile(photoFile)
            val finalFile = moveCapturedPhoto(photoFile, hash)
            val thumbnailPath = generateNativeThumbnail(activity, finalFile, hash)
            val result = buildPhotoResult(finalFile, originalName, mimeType, hash, thumbnailPath)
            publishPickedFile(result)
            invoke.resolve(result)
        } catch (error: Throwable) {
            Log.e(TAG, "[onCameraResult] 照片处理失败", error)
            invoke.reject("处理拍摄照片失败：${error.message}")
        }
    }

    protected fun publishFileStart(name: String, size: Long, mimeType: String) {
        val detail = JSObject().apply {
            put("name", name)
            put("size", size)
            put("mime", mimeType)
        }
        val safeDetail = escapeJsonForJsString(detail.toString())
        activity.runOnUiThread {
            webViewRef?.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('vcp-mobile-file-start', { detail: JSON.parse(\"$safeDetail\") }))",
                null
            )
        }
    }

    protected fun hashFile(file: java.io.File): String {
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        java.io.FileInputStream(file).use { input ->
            val buffer = ByteArray(65536)
            var read: Int
            while (input.read(buffer).also { read = it } != -1) {
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    private fun moveCapturedPhoto(photoFile: java.io.File, hash: String): java.io.File {
        val uploadsDir = java.io.File(activity.cacheDir, "uploads").apply { mkdirs() }
        val finalFile = java.io.File(uploadsDir, "$hash.jpg")
        if (finalFile.exists()) {
            photoFile.delete()
        } else {
            photoFile.renameTo(finalFile)
        }
        return finalFile
    }

    protected fun buildPhotoResult(
        file: java.io.File,
        name: String,
        mimeType: String,
        hash: String,
        thumbnailPath: String?
    ): JSObject {
        return JSObject().apply {
            put("path", file.absolutePath)
            put("name", name)
            put("mime", mimeType)
            put("size", file.length())
            put("hash", hash)
            if (thumbnailPath != null) put("thumbnailPath", thumbnailPath)
        }
    }

    protected fun publishPickedFile(result: JSObject) {
        val safeDetail = escapeJsonForJsString(result.toString())
        activity.runOnUiThread {
            webViewRef?.evaluateJavascript(
                "window.dispatchEvent(new CustomEvent('vcp-mobile-file-picked', { detail: JSON.parse(\"$safeDetail\") }))",
                null
            )
        }
    }
}
