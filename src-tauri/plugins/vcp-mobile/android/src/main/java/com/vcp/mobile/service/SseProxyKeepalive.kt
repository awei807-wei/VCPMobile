package com.vcp.mobile.service

import android.content.Context
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log

/** 仅在 helper 流运行期间持有物理锁并进行静音播放。 */
internal class SseProxyKeepalive(
    private val context: Context,
    private val physicalLocks: ForegroundPhysicalLocks = ForegroundPhysicalLocks(),
    releaseSchedulerOverride: ForegroundLockReleaseScheduler? = null,
) {
    companion object {
        private const val TAG = "VcpSseKeepalive"
    }

    private var mediaPlayer: MediaPlayer? = null
    private val releaseHandler = Handler(Looper.getMainLooper())
    private val releaseScheduler = releaseSchedulerOverride ?: ForegroundLockReleaseScheduler(
        postDelayed = { runnable, delay -> releaseHandler.postDelayed(runnable, delay) },
        removeCallbacks = { runnable -> releaseHandler.removeCallbacks(runnable) },
        releaseLocks = { physicalLocks.release() },
        logError = { message -> Log.e(TAG, message) },
    )

    @Synchronized
    fun update(hasRunningSessions: Boolean) {
        if (hasRunningSessions) {
            releaseScheduler.cancel()
            try {
                // ForegroundPhysicalLocks 在第二把锁获取失败时会回滚第一把，
                // 这里保留异常让调用方按 socket 断开语义清理本次 resume。
                physicalLocks.acquire(context)
            } catch (error: Throwable) {
                Log.e(TAG, "获取 SseProxy 物理保活锁失败：result=error")
                // acquire 失败时可能只回滚释放了部分句柄；把仍持有的句柄交给
                // 同一有界调度器，避免失败路径留下永久 WakeLock/WifiLock。
                releaseScheduler.release()
                throw error
            }
            startSilentPlayback()
        } else {
            releaseScheduler.release()
            stopSilentPlayback()
        }
    }

    @Synchronized
    fun release() {
        releaseScheduler.release()
        stopSilentPlayback()
    }

    @Synchronized
    private fun ensureSilentAudioFile(): java.io.File {
        val file = java.io.File(context.cacheDir, "silent.wav")
        if (file.exists() && file.length() > 0) {
            return file
        }
        return try {
            file.outputStream().use { out ->
                out.write(byteArrayOf(0x52, 0x49, 0x46, 0x46))
                out.write(byteArrayOf(0x64, 0x06, 0x00, 0x00))
                out.write(byteArrayOf(0x57, 0x41, 0x56, 0x45))
                out.write(byteArrayOf(0x66, 0x6d, 0x74, 0x20))
                out.write(byteArrayOf(0x10, 0x00, 0x00, 0x00))
                out.write(byteArrayOf(0x01, 0x00))
                out.write(byteArrayOf(0x01, 0x00))
                out.write(byteArrayOf(0x40, 0x1F, 0x00, 0x00))
                out.write(byteArrayOf(0x40, 0x1F, 0x00, 0x00))
                out.write(byteArrayOf(0x01, 0x00))
                out.write(byteArrayOf(0x08, 0x00))
                out.write(byteArrayOf(0x64, 0x61, 0x74, 0x61))
                out.write(byteArrayOf(0x40, 0x06, 0x00, 0x00))
                out.write(ByteArray(1600) { 0x80.toByte() })
            }
            Log.i(TAG, "已在缓存目录创建 silent.wav")
            file
        } catch (_error: Exception) {
            Log.e(TAG, "创建 silent.wav 失败：result=error")
            file
        }
    }

    @Synchronized
    private fun startSilentPlayback() {
        if (mediaPlayer != null) {
            return
        }
        try {
            val silentFile = ensureSilentAudioFile()
            mediaPlayer = MediaPlayer().apply {
                setDataSource(silentFile.absolutePath)
                isLooping = true
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                    setAudioAttributes(
                        AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                            .build()
                    )
                }
                prepare()
                start()
            }
            Log.i(TAG, "静音播放已启动")
        } catch (_error: Exception) {
            Log.e(TAG, "启动静音播放失败：result=error")
        }
    }

    @Synchronized
    private fun stopSilentPlayback() {
        mediaPlayer?.let { player ->
            try {
                if (player.isPlaying) {
                    player.stop()
                }
                player.release()
            } catch (_error: Exception) {
                Log.w(TAG, "停止静音播放失败：result=error")
            }
        }
        mediaPlayer = null
    }
}
