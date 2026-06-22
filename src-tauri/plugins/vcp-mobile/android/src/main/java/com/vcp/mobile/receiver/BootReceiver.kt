package com.vcp.mobile.receiver

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import com.vcp.mobile.service.StreamKeepaliveService

/**
 * 开机与包更新后的分布式保活恢复入口。
 *
 * Receiver 只能 best-effort 拉起 Android 前台保活提示；真正的 WebSocket 连接仍由
 * Tauri/Rust 核心在用户打开应用或系统允许进程恢复后执行 bootstrap + reconcile。
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        val action = intent?.action ?: return
        if (action != Intent.ACTION_BOOT_COMPLETED && action != Intent.ACTION_MY_PACKAGE_REPLACED) {
            return
        }

        if (!StreamKeepaliveService.isDistributedKeepalivePersisted(context)) {
            Log.i(TAG, "Distributed keepalive was not requested before $action; skipping boot recovery.")
            return
        }

        try {
            val serviceIntent = StreamKeepaliveService.createRecoveryIntent(context)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(serviceIntent)
            } else {
                context.startService(serviceIntent)
            }
            Log.i(TAG, "Distributed keepalive best-effort recovery requested after $action.")
        } catch (e: Exception) {
            Log.w(TAG, "System rejected distributed keepalive recovery after $action.", e)
        }
    }

    companion object {
        private const val TAG = "VcpBootReceiver"
    }
}
