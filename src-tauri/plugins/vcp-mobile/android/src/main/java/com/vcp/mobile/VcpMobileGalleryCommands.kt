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

open class VcpMobileGalleryCommands(activity: Activity) : VcpMobileShareCommands(activity) {
    // ==================================================================
    // 通用媒体导出与相册写入
    // ==================================================================
    @Command
    fun saveImageToGallery(invoke: Invoke) {
        val args = invoke.parseArgs(SaveImageArgs::class.java)
        if (args.sourceUrl.isBlank()) {
            invoke.reject("图片地址为空")
            return
        }

        fileIoExecutor.execute {
            try {
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
                    val writeGranted = ContextCompat.checkSelfPermission(activity, android.Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED
                    if (!writeGranted) {
                        invoke.reject("保存到相册需要储存空间权限")
                        return@execute
                    }
                }

                val loaded = loadImageBytes(args.sourceUrl)
                if (!loaded.mimeType.startsWith("image/")) {
                    invoke.reject("当前资源不是图片: ${loaded.mimeType}")
                    return@execute
                }

                val displayName = buildGalleryFileName(args.fileName, args.sourceUrl, loaded.mimeType)
                val savedUri = writeImageToGallery(loaded.bytes, displayName, loaded.mimeType)
                val result = JSObject().apply {
                    put("uri", savedUri.toString())
                    put("displayName", displayName)
                    put("mimeType", loaded.mimeType)
                    put("size", loaded.bytes.size)
                }
                invoke.resolve(result)
            } catch (e: Throwable) {
                Log.e(TAG, "保存图片到相册失败", e)
                invoke.reject("保存图片失败: ${e.message}")
            }
        }
    }

    @Command
    fun saveImageFromPath(invoke: Invoke) {
        val args = invoke.parseArgs(SaveImageFromPathArgs::class.java)
        if (args.imagePath.isBlank()) {
            invoke.reject("物理文件路径为空")
            return
        }

        // 1. 保留已验证的 canonical File，异步任务不得重新使用未经校验的原路径。
        val safeFile = resolveSafeLocalFile(activity, args.imagePath)
        if (safeFile == null) {
            invoke.reject("非法的本地文件读取边界，已被安全沙箱拒绝")
            return
        }

        fileIoExecutor.execute {
            saveImageFromPathTask(args, safeFile, invoke)
        }
    }

    private fun saveImageFromPathTask(
        args: SaveImageFromPathArgs,
        file: java.io.File,
        invoke: Invoke,
    ) {
        try {
            if (!file.exists()) {
                invoke.reject("本地临时文件不存在")
                return
            }
            if (!ensureGalleryWritePermission(invoke)) return

            val bytes = file.readBytes()
            val mimeType = sniffImageMime(bytes, file.name, true)
            if (!mimeType.startsWith("image/")) {
                invoke.reject("当前资源不是图片: $mimeType")
                return
            }
            val displayName = buildGalleryFileName(args.fileName, file.name, mimeType)
            val savedUri = writeImageToGallery(bytes, displayName, mimeType)
            invoke.resolve(buildGalleryResult(savedUri, displayName, mimeType, bytes.size))
        } catch (e: Throwable) {
            Log.e(TAG, "从路径保存图片失败", e)
            invoke.reject("保存图片失败: ${e.message}")
        } finally {
            deleteTemporaryGalleryFile(file)
        }
    }

    private fun ensureGalleryWritePermission(invoke: Invoke): Boolean {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) return true
        val granted = ContextCompat.checkSelfPermission(
            activity,
            android.Manifest.permission.WRITE_EXTERNAL_STORAGE,
        ) == PackageManager.PERMISSION_GRANTED
        if (!granted) {
            invoke.reject("保存到相册需要储存空间权限")
            return false
        }
        return true
    }

    private fun buildGalleryResult(
        savedUri: Uri,
        displayName: String,
        mimeType: String,
        size: Int,
    ): JSObject = JSObject().apply {
        put("uri", savedUri.toString())
        put("displayName", displayName)
        put("mimeType", mimeType)
        put("size", size)
    }

    private fun deleteTemporaryGalleryFile(file: java.io.File) {
        try {
            if (file.exists()) file.delete()
        } catch (error: Exception) {
            Log.e(TAG, "清理临时保存图片失败", error)
        }
    }

    protected data class LoadedImage(val bytes: ByteArray, val mimeType: String)

    protected fun loadImageBytes(sourceUrl: String): LoadedImage {
        if (sourceUrl.startsWith("data:", ignoreCase = true)) {
            return loadDataUrlImage(sourceUrl)
        }

        if (sourceUrl.startsWith("content:", ignoreCase = true)) {
            val uri = Uri.parse(sourceUrl)
            val mime = activity.contentResolver.getType(uri) ?: mimeFromSource(sourceUrl)
            val bytes = activity.contentResolver.openInputStream(uri).use { input ->
                readBytesLimited(input ?: throw IllegalStateException("无法读取 content 图片"))
            }
            return LoadedImage(bytes, sniffImageMime(bytes, mime, isLocal = true))
        }

        if (sourceUrl.startsWith("file:", ignoreCase = true) || sourceUrl.startsWith("/")) {
            val path = if (sourceUrl.startsWith("file:", ignoreCase = true)) {
                Uri.parse(sourceUrl).path ?: sourceUrl.removePrefix("file://")
            } else {
                sourceUrl
            }

            // 💥 安全防线：本地路径强制进行沙箱越权校验
            val file = resolveSafeLocalFile(activity, path)
            if (file == null) {
                throw SecurityException("越权拒绝：禁止读取沙箱外部资源")
            }

            val bytes = file.inputStream().use { readBytesLimited(it) }
            return LoadedImage(bytes, sniffImageMime(bytes, mimeFromSource(file.name), isLocal = true))
        }

        return loadNetworkImage(sourceUrl)
    }

    protected fun loadNetworkImage(sourceUrl: String): LoadedImage {
        val connection = (URL(sourceUrl).openConnection() as HttpURLConnection).apply {
            connectTimeout = 5000  // 💥 优化：降低至5秒
            readTimeout = 10000    // 💥 优化：降低至10秒
            instanceFollowRedirects = true
            setRequestProperty("User-Agent", "VCPMobile/1.0")
        }

        try {
            val status = connection.responseCode
            if (status !in 200..299) {
                throw IllegalStateException("HTTP $status")
            }
            val contentType = connection.contentType?.substringBefore(";")?.lowercase(Locale.US)
            val bytes = connection.inputStream.use { readBytesLimited(it) }
            return LoadedImage(bytes, sniffImageMime(bytes, contentType ?: mimeFromSource(sourceUrl), isLocal = false))
        } finally {
            connection.disconnect()
        }
    }

    protected fun loadDataUrlImage(dataUrl: String): LoadedImage {
        val commaIndex = dataUrl.indexOf(',')
        if (commaIndex <= 0) throw IllegalArgumentException("无效的 data URL")

        val header = dataUrl.substring(5, commaIndex)
        val mime = header.substringBefore(";").ifBlank { "application/octet-stream" }.lowercase(Locale.US)
        val payload = dataUrl.substring(commaIndex + 1)
        val bytes = if (header.contains(";base64", ignoreCase = true)) {
            Base64.decode(payload, Base64.DEFAULT)
        } else {
            URLDecoder.decode(payload, "UTF-8").toByteArray(Charsets.UTF_8)
        }
        return LoadedImage(bytes, sniffImageMime(bytes, mime, isLocal = false))
    }

    protected fun readBytesLimited(input: InputStream, maxBytes: Int = 50 * 1024 * 1024): ByteArray {
        val output = ByteArrayOutputStream()
        val buffer = ByteArray(64 * 1024)
        var total = 0
        while (true) {
            val read = input.read(buffer)
            if (read == -1) break
            total += read
            if (total > maxBytes) {
                throw IllegalArgumentException("图片过大，超过 50MB")
            }
            output.write(buffer, 0, read)
        }
        return output.toByteArray()
    }

    protected fun sniffImageMime(bytes: ByteArray, fallback: String, isLocal: Boolean): String {
        val normalized = fallback.substringBefore(";").lowercase(Locale.US)

        // 💥 安全校验：若是网络资源可信任 content-type，若是本地绝对物理路径，必须强行分析文件头二进制，防止伪造扩展名泄漏明文
        if (!isLocal && normalized.startsWith("image/")) {
            return normalized
        }

        if (bytes.size >= 8 && bytes[0] == 0x89.toByte() && bytes[1] == 0x50.toByte() && bytes[2] == 0x4E.toByte() && bytes[3] == 0x47.toByte()) return "image/png"
        if (bytes.size >= 3 && bytes[0] == 0xFF.toByte() && bytes[1] == 0xD8.toByte() && bytes[2] == 0xFF.toByte()) return "image/jpeg"
        if (bytes.size >= 6 && String(bytes, 0, 6, Charsets.US_ASCII).startsWith("GIF")) return "image/gif"
        if (bytes.size >= 12 && String(bytes, 0, 4, Charsets.US_ASCII) == "RIFF" && String(bytes, 8, 4, Charsets.US_ASCII) == "WEBP") return "image/webp"
        if (bytes.size >= 2 && bytes[0] == 0x42.toByte() && bytes[1] == 0x4D.toByte()) return "image/bmp"

        val sample = bytes.take(256).toByteArray().toString(Charsets.UTF_8).trimStart()
        if (sample.startsWith("<svg", ignoreCase = true) || sample.startsWith("<?xml", ignoreCase = true)) return "image/svg+xml"

        // 本地读取兜底降级：非图片格式的敏感文件一律设为 application/octet-stream，从而在 saveImageToGallery 判定 mime.startsWith("image/") 时被拦截
        if (isLocal) {
            return "application/octet-stream"
        }
        return normalized
    }

    protected fun mimeFromSource(source: String): String {
        val clean = source.substringBefore("?").substringBefore("#")
        val ext = clean.substringAfterLast('.', "").lowercase(Locale.US)
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: when (ext) {
            "jpg", "jpeg" -> "image/jpeg"
            "png" -> "image/png"
            "gif" -> "image/gif"
            "webp" -> "image/webp"
            "svg" -> "image/svg+xml"
            "bmp" -> "image/bmp"
            "avif" -> "image/avif"
            "heic", "heif" -> "image/heic"
            else -> "application/octet-stream"
        }
    }

    protected fun extensionForMime(mimeType: String): String {
        return when (mimeType.lowercase(Locale.US)) {
            "image/jpeg" -> "jpg"
            "image/png" -> "png"
            "image/gif" -> "gif"
            "image/webp" -> "webp"
            "image/svg+xml" -> "svg"
            "image/bmp" -> "bmp"
            "image/avif" -> "avif"
            "image/heic" -> "heic"
            "image/heif" -> "heif"
            else -> "png"
        }
    }

    protected fun buildGalleryFileName(providedName: String?, sourceUrl: String, mimeType: String): String {
        val fromUrl = if (!sourceUrl.startsWith("data:", ignoreCase = true) && !sourceUrl.startsWith("blob:", ignoreCase = true)) {
            try {
                Uri.parse(sourceUrl).lastPathSegment?.let { URLDecoder.decode(it, "UTF-8") }
            } catch (_: Exception) {
                null
            }
        } else {
            null
        }

        val timestamp = SimpleDateFormat("yyyyMMdd_HHmmss", Locale.US).format(Date())
        val rawName = providedName?.takeIf { it.isNotBlank() } ?: fromUrl ?: "vcp_image_$timestamp"
        val sanitized = rawName.replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001F]"), "_").trim().ifBlank { "vcp_image_$timestamp" }
        val base = sanitized.substringBeforeLast('.', sanitized).take(96).ifBlank { "vcp_image_$timestamp" }
        val ext = sanitized.substringAfterLast('.', "").lowercase(Locale.US).takeIf { it.isNotBlank() } ?: extensionForMime(mimeType)
        return "$base.$ext"
    }

    protected fun writeImageToGallery(bytes: ByteArray, displayName: String, mimeType: String): Uri {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val resolver = activity.contentResolver
            val values = ContentValues().apply {
                put(MediaStore.Images.Media.DISPLAY_NAME, displayName)
                put(MediaStore.Images.Media.MIME_TYPE, mimeType)
                put(MediaStore.Images.Media.RELATIVE_PATH, "${Environment.DIRECTORY_PICTURES}/VCPMobile")
                put(MediaStore.Images.Media.IS_PENDING, 1)
            }
            val uri = resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values)
                ?: throw IllegalStateException("无法创建相册图片")
            try {
                resolver.openOutputStream(uri)?.use { it.write(bytes) }
                    ?: throw IllegalStateException("无法写入相册图片")
                values.clear()
                values.put(MediaStore.Images.Media.IS_PENDING, 0)
                resolver.update(uri, values, null, null)
                return uri
            } catch (e: Throwable) {
                resolver.delete(uri, null, null)
                throw e
            }
        }

        val picturesDir = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_PICTURES)
        val appDir = java.io.File(picturesDir, "VCPMobile").apply { mkdirs() }
        var outputFile = java.io.File(appDir, displayName)
        if (outputFile.exists()) {
            val base = displayName.substringBeforeLast('.', displayName)
            val ext = displayName.substringAfterLast('.', "")
            var index = 1
            do {
                outputFile = java.io.File(appDir, if (ext.isBlank()) "${base}_$index" else "${base}_$index.$ext")
                index += 1
            } while (outputFile.exists())
        }

        java.io.FileOutputStream(outputFile).use { it.write(bytes) }
        MediaScannerConnection.scanFile(activity, arrayOf(outputFile.absolutePath), arrayOf(mimeType), null)
        return Uri.fromFile(outputFile)
    }
}
