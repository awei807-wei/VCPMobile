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

open class VcpMobilePickedFileCommands(activity: Activity) : VcpMobileFilePickerCommands(activity) {
    private data class PickedSource(
        val name: String,
        val size: Long,
        val mimeType: String
    )

    private data class PreparedPickedFile(
        val file: java.io.File,
        val name: String,
        val mimeType: String,
        val size: Long,
        val hash: String,
        val extension: String
    )

    @ActivityCallback
    fun onPickFileResult(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode != Activity.RESULT_OK) {
            Log.w(TAG, "[onPickFileResult] 文件选择已取消或失败")
            invoke.reject("已取消")
            return
        }

        val uri = result.data?.data
        if (uri == null) {
            Log.w(TAG, "[onPickFileResult] 选择的 URI 为空")
            invoke.reject("未选择文件")
            return
        }
        fileIoExecutor.execute { processPickedFile(uri, invoke) }
    }

    private fun processPickedFile(uri: Uri, invoke: Invoke) {
        var currentTempFile: java.io.File? = null
        try {
            val source = readPickedSource(uri)
            Log.i(TAG, "[onPickFileResult] 开始处理文件：${source.name}")
            publishFileStart(source.name, source.size, source.mimeType)
            val copied = copyPickedFile(uri, source)
            currentTempFile = copied.file
            val prepared = transcodePickedFile(copied)
            currentTempFile = prepared.file
            val finalFile = movePickedFile(prepared)
            val thumbnail = if (prepared.mimeType.startsWith("image/")) {
                generateNativeThumbnail(activity, finalFile, prepared.hash)
            } else {
                null
            }
            val result = buildPickedResult(finalFile, prepared, thumbnail)
            publishPickedFile(result)
            invoke.resolve(result)
        } catch (error: Throwable) {
            Log.e(TAG, "[onPickFileResult] 文件处理失败", error)
            currentTempFile?.delete()
            invoke.reject("处理所选文件失败：${error.message}")
        }
    }

    private fun readPickedSource(uri: Uri): PickedSource {
        val resolver = activity.contentResolver
        var name = "unknown"
        var size = 0L
        resolver.query(uri, null, null, null, null)?.use { cursor ->
            val nameIndex = cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
            val sizeIndex = cursor.getColumnIndex(android.provider.OpenableColumns.SIZE)
            if (cursor.moveToFirst()) {
                if (nameIndex != -1) name = cursor.getString(nameIndex)
                if (sizeIndex != -1) size = cursor.getLong(sizeIndex)
            }
        }
        val mimeType = resolver.getType(uri) ?: "application/octet-stream"
        return PickedSource(name, size, mimeType)
    }

    private fun copyPickedFile(uri: Uri, source: PickedSource): PreparedPickedFile {
        val uploadsDir = java.io.File(activity.cacheDir, "uploads").apply { mkdirs() }
        val tempFile = java.io.File(uploadsDir, "pick_${System.currentTimeMillis()}_temp")
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        copyPickedStream(uri, tempFile, source, digest)
        val hash = digest.digest().joinToString("") { "%02x".format(it) }
        val extension = java.io.File(source.name).extension.let {
            if (it.isEmpty()) "" else ".$it"
        }
        return PreparedPickedFile(tempFile, source.name, source.mimeType, source.size, hash, extension)
    }

    private fun copyPickedStream(
        uri: Uri,
        target: java.io.File,
        source: PickedSource,
        digest: java.security.MessageDigest
    ) {
        val resolver = activity.contentResolver
        resolver.openInputStream(uri).use { input ->
            if (input == null) throw IllegalStateException("无法打开所选文件")
            java.io.FileOutputStream(target).use { output ->
                val buffer = ByteArray(65536)
                var totalRead = 0L
                var lastReport = System.currentTimeMillis()
                var read: Int
                while (input.read(buffer).also { read = it } != -1) {
                    output.write(buffer, 0, read)
                    digest.update(buffer, 0, read)
                    totalRead += read
                    if (System.currentTimeMillis() - lastReport > 200) {
                        lastReport = System.currentTimeMillis()
                        publishFileProgress(source, totalRead)
                    }
                }
            }
        }
    }

    private fun publishFileProgress(source: PickedSource, loaded: Long) {
        val progress = if (source.size > 0) ((loaded.toDouble() / source.size) * 100).toInt() else 0
        val detail = JSObject().apply {
            put("loaded", loaded)
            put("total", source.size)
            put("progress", progress)
            put("name", source.name)
            put("mime", source.mimeType)
        }
        val safeDetail = escapeJsonForJsString(detail.toString())
        val script = "window.dispatchEvent(new CustomEvent('vcp-mobile-file-progress', { detail: JSON.parse(\"$safeDetail\") }))"
        activity.runOnUiThread { webViewRef?.evaluateJavascript(script, null) }
    }

    private fun transcodePickedFile(source: PreparedPickedFile): PreparedPickedFile {
        val ext = source.name.substringAfterLast(".").lowercase()
        val sdk = Build.VERSION.SDK_INT
        val unsupportedVideo = listOf("mkv", "avi", "flv", "wmv", "ts").contains(ext)
        val unsupportedAudio = listOf("wma", "aiff").contains(ext)
        val unsupportedHeic = (ext == "heic" || ext == "heif") && sdk < 28
        val unsupportedAvif = ext == "avif" && sdk < 31
        val unsupportedOpus = ext == "opus" && sdk < 29
        if (!unsupportedVideo && !unsupportedAudio && !unsupportedHeic && !unsupportedAvif && !unsupportedOpus) {
            return source
        }

        val audioOnly = unsupportedAudio || unsupportedOpus || (ext == "ogg" && sdk < 29)
        val imageOnly = unsupportedHeic || unsupportedAvif
        val suffix = if (audioOnly) "m4a" else if (imageOnly) "jpg" else "mp4"
        val output = java.io.File(activity.cacheDir, "uploads/transcoded_${System.currentTimeMillis()}.$suffix")
        return try {
            runTranscode(source.file, output, audioOnly, imageOnly)
            source.file.delete()
            source.copy(
                file = output,
                name = source.name.substringBeforeLast(".") + ".$suffix",
                mimeType = if (audioOnly) "audio/mp4" else if (imageOnly) "image/jpeg" else "video/mp4",
                hash = hashFile(output),
                extension = ".$suffix"
            )
        } catch (error: Throwable) {
            output.delete()
            throw error
        }
    }

    private fun runTranscode(
        input: java.io.File,
        output: java.io.File,
        audioOnly: Boolean,
        imageOnly: Boolean
    ) {
        val latch = CountDownLatch(1)
        var failure: Throwable? = null
        activity.runOnUiThread {
            try {
                val request = TransformationRequest.Builder()
                    .setVideoMimeType(if (!audioOnly && !imageOnly) MimeTypes.VIDEO_H264 else null)
                    .setAudioMimeType(MimeTypes.AUDIO_AAC)
                    .build()
                val transformer = Transformer.Builder(activity)
                    .setTransformationRequest(request)
                    .addListener(object : Transformer.Listener {
                        override fun onCompleted(composition: Composition, result: ExportResult) {
                            latch.countDown()
                        }

                        override fun onError(
                            composition: Composition,
                            result: ExportResult,
                            exception: ExportException
                        ) {
                            failure = exception
                            latch.countDown()
                        }
                    })
                    .build()
                val mediaItem = MediaItem.fromUri(Uri.fromFile(input))
                val edited = EditedMediaItem.Builder(mediaItem).setRemoveAudio(false).build()
                transformer.start(edited, output.absolutePath)
            } catch (error: Throwable) {
                failure = error
                latch.countDown()
            }
        }
        if (!latch.await(300, TimeUnit.SECONDS)) {
            throw java.util.concurrent.TimeoutException("转码超过 5 分钟")
        }
        failure?.let { throw it }
    }

    private fun movePickedFile(source: PreparedPickedFile): java.io.File {
        val finalFile = java.io.File(activity.cacheDir, "uploads/${source.hash}${source.extension}")
        if (finalFile.exists()) {
            source.file.delete()
        } else {
            source.file.renameTo(finalFile)
        }
        return finalFile
    }

    private fun buildPickedResult(
        file: java.io.File,
        source: PreparedPickedFile,
        thumbnail: String?
    ): JSObject {
        val result = JSObject().apply {
            put("path", file.absolutePath)
            put("name", source.name)
            put("mime", source.mimeType)
            put("size", if (source.size > 0) source.size else file.length())
            put("hash", source.hash)
            if (thumbnail != null) put("thumbnailPath", thumbnail)
        }
        Log.i(TAG, "[onPickFileResult] 文件处理完成：${file.absolutePath}")
        return result
    }
}
