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

@InvokeArg
class RequestPermissionArgs {
    lateinit var type: String
}
@InvokeArg
class OpenFileArgs {
    lateinit var path: String
}

@InvokeArg
class ShareFileArgs {
    lateinit var path: String
    var title: String? = null
}

@InvokeArg
class PickFileArgs {
    var mode: String = "file"
}

@InvokeArg
class SaveImageArgs {
    lateinit var sourceUrl: String
    var fileName: String? = null
}

@InvokeArg
class SaveImageFromPathArgs {
    lateinit var imagePath: String
    var fileName: String? = null
}

@InvokeArg
class CaptureWindowSnapshotArgs {
    var maxWidth: Int = 200 // 与 Rust 侧默认参数对齐
    var quality: Int = 64  // 与 Rust 侧默认参数对齐
}

@InvokeArg
class ProcessImageArgs {
    lateinit var path: String
}

@InvokeArg
class ProcessVideoArgs {
    lateinit var path: String
}

@InvokeArg
class ProcessAudioArgs {
    lateinit var path: String
}

@InvokeArg
class UpdateDownloadNotifArgs {
    var progress: Int = 0
    var text: String? = null
}

@InvokeArg
class ShowSystemNotificationArgs {
    lateinit var title: String
    lateinit var body: String
}

@InvokeArg
class ToggleFloatingBallArgs {
    var show: Boolean = false
}

@InvokeArg
class ProcessSharedFileArgs {
    lateinit var cachePath: String
    var mimeType: String? = null
    lateinit var fileName: String
}

@InvokeArg
class GetSensorDataArgs {
    lateinit var type: String
}

@InvokeArg
class RunRootCommandArgs {
    lateinit var command: String
}

@InvokeArg
class AcquireForegroundArgs {
    lateinit var tag: String
    var priority: Int = 0
    lateinit var label: String
    var screenKeepOn: Boolean = false
}

@InvokeArg
class ReleaseForegroundArgs {
    lateinit var tag: String
}

@InvokeArg
class WriteClipboardArgs {
    lateinit var content: String
}

@InvokeArg
class SendLocalNotificationArgs {
    lateinit var title: String
    lateinit var body: String
}
