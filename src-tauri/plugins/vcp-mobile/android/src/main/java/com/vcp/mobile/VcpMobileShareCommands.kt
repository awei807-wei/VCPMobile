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

open class VcpMobileShareCommands(activity: Activity) : VcpMobilePickedFileCommands(activity) {
    // ==================================================================
    // 外部分享文件处理（不弹选择器，直接处理缓存文件）
    // ==================================================================
    @Command
    fun processSharedFile(invoke: Invoke) {
        val args = invoke.parseArgs(ProcessSharedFileArgs::class.java)
        if (args.cachePath.isBlank()) {
            invoke.reject("缓存文件路径为空")
            return
        }
        val sharedDirectory = java.io.File(activity.cacheDir, "shared")
        val sourceFile = resolveCanonicalRegularFileInDirectory(
            java.io.File(args.cachePath),
            sharedDirectory,
        )
        if (sourceFile == null) {
            invoke.reject("分享文件必须位于应用 cache/shared 且为普通文件")
            return
        }
        fileIoExecutor.execute { processSharedFileTask(args, sourceFile, invoke) }
    }

    private fun processSharedFileTask(
        args: ProcessSharedFileArgs,
        sourceFile: java.io.File,
        invoke: Invoke,
    ) {
        try {
            if (!sourceFile.exists() || !sourceFile.isFile) {
                invoke.reject("缓存路径中未找到分享文件：${sourceFile.path}")
                return
            }
            val mimeType = resolveSharedMimeType(args.mimeType, sourceFile)
            Log.i(
                TAG,
                "[processSharedFile] 正在处理分享文件：${args.fileName}（size=${sourceFile.length()}，mime=$mimeType）",
            )
            dispatchSharedFileStart(args.fileName, sourceFile.length(), mimeType)
            val copy = copySharedFile(sourceFile, args.fileName)
            val thumbnail = if (mimeType.startsWith("image/")) {
                generateNativeThumbnail(activity, copy.finalFile, copy.hash)
            } else {
                null
            }
            val result = buildSharedFileResult(copy, args.fileName, mimeType, thumbnail)
            dispatchSharedFilePicked(copy, args.fileName, mimeType, thumbnail)
            Log.i(
                TAG,
                "[processSharedFile] 处理完成：path=${copy.finalFile.absolutePath}，hash=${copy.hash}",
            )
            invoke.resolve(result)
        } catch (error: Throwable) {
            Log.e(TAG, "[processSharedFile] 处理失败", error)
            invoke.reject("处理分享文件失败：${error.message}")
        }
    }

    private data class SharedFileCopy(
        val finalFile: java.io.File,
        val hash: String,
    )

    private fun resolveSharedMimeType(
        rawMimeType: String?,
        sourceFile: java.io.File,
    ): String {
        if (!rawMimeType.isNullOrBlank()) return rawMimeType
        val extension = sourceFile.extension.lowercase()
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)
            ?: "application/octet-stream"
    }

    private fun copySharedFile(
        sourceFile: java.io.File,
        originalName: String,
    ): SharedFileCopy {
        val uploadsDir = java.io.File(activity.cacheDir, "uploads").apply { mkdirs() }
        val tempFile = java.io.File(uploadsDir, "shared_${System.currentTimeMillis()}_temp")
        try {
            val digest = java.security.MessageDigest.getInstance("SHA-256")
            sourceFile.inputStream().use { input ->
                java.io.FileOutputStream(tempFile).use { output ->
                    copyAndDigest(input, output, digest)
                }
            }
            val hash = digest.digest().joinToString("") { "%02x".format(it) }
            val extension = java.io.File(originalName).extension.let {
                if (it.isEmpty()) "" else ".${it}"
            }
            val finalFile = java.io.File(uploadsDir, "$hash$extension")
            if (finalFile.exists()) {
                tempFile.delete()
            } else if (!tempFile.renameTo(finalFile)) {
                throw IllegalStateException("无法发布分享文件")
            }
            return SharedFileCopy(finalFile, hash)
        } catch (error: Throwable) {
            tempFile.delete()
            throw error
        }
    }

    private fun copyAndDigest(
        input: InputStream,
        output: java.io.OutputStream,
        digest: java.security.MessageDigest,
    ) {
        val buffer = ByteArray(65536)
        var bytesRead = input.read(buffer)
        while (bytesRead != -1) {
            output.write(buffer, 0, bytesRead)
            digest.update(buffer, 0, bytesRead)
            bytesRead = input.read(buffer)
        }
    }

    private fun buildSharedFileResult(
        copy: SharedFileCopy,
        originalName: String,
        mimeType: String,
        thumbnail: String?,
    ): JSObject = JSObject().apply {
        put("path", copy.finalFile.absolutePath)
        put("name", originalName)
        put("mime", mimeType)
        put("size", copy.finalFile.length())
        put("hash", copy.hash)
        if (thumbnail != null) put("thumbnailPath", thumbnail)
    }

    private fun dispatchSharedFileStart(name: String, size: Long, mimeType: String) {
        val detail = JSObject().apply {
            put("name", name)
            put("size", size)
            put("mime", mimeType)
        }
        dispatchSharedFileEvent("vcp-mobile-file-start", detail)
    }

    private fun dispatchSharedFilePicked(
        copy: SharedFileCopy,
        name: String,
        mimeType: String,
        thumbnail: String?,
    ) {
        val detail = JSObject().apply {
            put("path", copy.finalFile.absolutePath)
            put("name", name)
            put("mime", mimeType)
            put("size", copy.finalFile.length())
            put("hash", copy.hash)
            put("thumbnailPath", thumbnail ?: org.json.JSONObject.NULL)
        }
        dispatchSharedFileEvent("vcp-mobile-file-picked", detail)
    }

    private fun dispatchSharedFileEvent(eventName: String, detail: JSObject) {
        val encoded = escapeJsonForJsString(detail.toString())
        val script = "window.dispatchEvent(new CustomEvent('$eventName', { detail: JSON.parse(\"$encoded\") }))"
        activity.runOnUiThread { webViewRef?.evaluateJavascript(script, null) }
    }

    @Command
    fun openFile(invoke: Invoke) {
        val args = invoke.parseArgs(OpenFileArgs::class.java)
        val path = args.path
        if (path.isEmpty()) {
            invoke.reject("文件路径为空")
            return
        }

        fileIoExecutor.execute { openFileTask(path, invoke) }
    }

    private fun openFileTask(path: String, invoke: Invoke) {
        var verifiedFile: java.io.File? = null
        try {
            val context = activity
            val file = resolveSafeLocalFile(context, path)
            if (file == null) {
                invoke.reject("安全拒绝：禁止打开沙箱外部的敏感文件")
                return
            }
            verifiedFile = file
            if (!file.exists()) {
                invoke.reject("文件不存在: ${file.path}")
                return
            }
            val mimeType = MimeTypeMap.getSingleton()
                .getMimeTypeFromExtension(file.extension.lowercase()) ?: "*/*"
            Log.i(TAG, "[openFile] 正在打开文件：${file.absolutePath}（mime=$mimeType）")
            context.startActivity(buildOpenFileIntent(context, file, mimeType))
            invoke.resolve()
        } catch (error: android.content.ActivityNotFoundException) {
            val extension = verifiedFile?.extension?.lowercase().orEmpty()
            Log.e(TAG, "[openFile] 没有可处理 .$extension 文件类型的 Activity", error)
            invoke.reject("您的手机上未安装能打开此类文件 (.$extension) 的应用，请先安装相关阅读器 (如 WPS Office)。")
        } catch (error: Throwable) {
            Log.e(TAG, "[openFile] 原生文件查看失败", error)
            invoke.reject("打开文件失败: ${error.message}")
        }
    }

    private fun buildOpenFileIntent(
        context: Context,
        file: java.io.File,
        mimeType: String,
    ): Intent {
        val uri = try {
            FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", file)
        } catch (error: Exception) {
            Log.w(TAG, "[openFile] 回退到 opener FileProvider authority", error)
            FileProvider.getUriForFile(context, "${context.packageName}.opener.fileprovider", file)
        }
        return Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, mimeType)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
    }

    @Command
    fun shareFile(invoke: Invoke) {
        val args = invoke.parseArgs(ShareFileArgs::class.java)
        val path = args.path
        if (path.isBlank()) {
            invoke.reject("文件路径为空")
            return
        }

        fileIoExecutor.execute {
            try {
                val context = activity
                val file = resolveSafeLocalFile(context, path)
                if (file == null) {
                    invoke.reject("安全拒绝：禁止分享沙箱外部文件")
                    return@execute
                }

                if (!file.exists() || !file.isFile) {
                    invoke.reject("文件不存在: ${file.path}")
                    return@execute
                }

                val mimeType = MimeTypeMap.getSingleton()
                    .getMimeTypeFromExtension(file.extension.lowercase()) ?: "application/octet-stream"
                val uri = try {
                    FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", file)
                } catch (error: Exception) {
                    Log.w(TAG, "[shareFile] 回退到 opener FileProvider authority", error)
                    FileProvider.getUriForFile(context, "${context.packageName}.opener.fileprovider", file)
                }

                val shareIntent = Intent(Intent.ACTION_SEND).apply {
                    type = mimeType
                    putExtra(Intent.EXTRA_STREAM, uri)
                    clipData = android.content.ClipData.newUri(context.contentResolver, file.name, uri)
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
                val chooser = Intent.createChooser(
                    shareIntent,
                    args.title?.takeIf { it.isNotBlank() } ?: "分享文件"
                ).apply {
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                context.startActivity(chooser)
                invoke.resolve()
            } catch (error: Throwable) {
                Log.e(TAG, "[shareFile] 分享失败", error)
                invoke.reject("分享文件失败: ${error.message}")
            }
        }
    }

    @Command
    fun getProcessExitDiagnostics(invoke: Invoke) {
        fileIoExecutor.execute {
            try {
                invoke.resolve(CrashDiagnostics.collectHistoricalExitReasons(activity.applicationContext))
            } catch (error: Throwable) {
                Log.e(TAG, "[getProcessExitDiagnostics] 读取失败", error)
                invoke.reject("读取 Android 退出诊断失败: ${error.message}")
            }
        }
    }
}
