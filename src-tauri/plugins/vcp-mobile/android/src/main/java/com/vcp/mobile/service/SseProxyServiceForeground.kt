package com.vcp.mobile.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat
import com.vcp.mobile.R

/** 管理 SSE helper 的前台通知与生命周期入口。 */
internal class SseProxyServiceForeground(private val service: Service) {
    companion object {
        private const val TAG = "VcpSseProxy"
        private const val ALERT_CHANNEL_ID = "vcp_agent_alerts"
    }

    fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return
        }
        val manager = service.getSystemService(NotificationManager::class.java) ?: return
        val serviceChannel = NotificationChannel(
            SseProxyService.CHANNEL_ID,
            "VCP 后台连接助手",
            NotificationManager.IMPORTANCE_HIGH
        ).apply {
            description = "维持后台稳定的 AI 对话流式连接"
            setShowBadge(false)
            enableVibration(false)
            setSound(null, null)
        }
        manager.createNotificationChannel(serviceChannel)
        val alertChannel = NotificationChannel(
            ALERT_CHANNEL_ID,
            "智能体消息提醒",
            NotificationManager.IMPORTANCE_HIGH
        ).apply {
            description = "接收智能体回复完成或中断的通知"
            enableLights(true)
            lightColor = android.graphics.Color.BLUE
            enableVibration(true)
        }
        manager.createNotificationChannel(alertChannel)
    }

    fun buildServiceNotification(): Notification {
        val intent = buildOpenIntent()
        val pendingIntent = PendingIntent.getActivity(
            service,
            0,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
        val builder = NotificationCompat.Builder(service, SseProxyService.CHANNEL_ID)
            .setContentTitle("VCP 连接助手")
            .setContentText("正在后台托管 AI 对话流式连接...")
            .setSmallIcon(R.drawable.ic_vcp_notification)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(pendingIntent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(Notification.CATEGORY_SERVICE)
            .addAction(R.drawable.ic_vcp_notification, "打开", pendingIntent)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }
        return builder.build()
    }

    fun promote(notification: Notification): Boolean {
        return try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                service.startForeground(
                    SseProxyService.NOTIFICATION_ID_SERVICE,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
                )
            } else {
                service.startForeground(SseProxyService.NOTIFICATION_ID_SERVICE, notification)
            }
            true
        } catch (_error: Exception) {
            Log.e(TAG, "startForeground 失败：result=error")
            false
        }
    }

    fun remove() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            service.stopForeground(Service.STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            service.stopForeground(true)
        }
    }

    private fun buildOpenIntent(): Intent {
        return try {
            val activityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(service, activityClass).apply {
                flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            }
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(service.packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }
    }
}
