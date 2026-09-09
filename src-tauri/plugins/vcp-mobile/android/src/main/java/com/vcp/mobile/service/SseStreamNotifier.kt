package com.vcp.mobile.service

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat
import com.vcp.mobile.R

/** 发布完成通知，但不拥有流状态。 */
class SseStreamNotifier(
    private val context: Context,
    private val appInForeground: () -> Boolean
) {
    companion object {
        private const val TAG = "VcpSseNotifier"
        private const val ALERT_CHANNEL = "vcp_agent_alerts"
    }

    fun show(session: SseStreamSession, isSuccess: Boolean, errorMsg: String?) {
        if (appInForeground()) {
            Log.d(TAG, "应用位于前台，跳过通知")
            return
        }
        if (isManualCancellation(isSuccess, errorMsg)) {
            return
        }

        val agentName = session.contextJson?.optString("agentName")
            ?.takeIf(String::isNotBlank) ?: "智能体"
        val title = buildTitle(agentName, isSuccess)
        val content = buildContent(session, isSuccess, errorMsg)
        val notificationManager = context.getSystemService(Context.NOTIFICATION_SERVICE)
            as? NotificationManager ?: return
        val pendingIntent = buildPendingIntent(session)
        val notification = NotificationCompat.Builder(context, ALERT_CHANNEL)
            .setContentTitle(title)
            .setContentText(content)
            .setSmallIcon(R.drawable.ic_vcp_notification)
            .setAutoCancel(true)
            .setContentIntent(pendingIntent)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setDefaults(Notification.DEFAULT_ALL)
            .build()
        notificationManager.notify(notificationId(session), notification)
    }

    private fun isManualCancellation(isSuccess: Boolean, errorMsg: String?): Boolean {
        if (isSuccess || errorMsg == null) {
            return false
        }
        return errorMsg.contains("cancel", ignoreCase = true) ||
            errorMsg.contains("close", ignoreCase = true)
    }

    private fun buildTitle(agentName: String, isSuccess: Boolean): String {
        return if (isSuccess) "✨ $agentName 已回复" else "⚠️ 与 $agentName 的对话中断"
    }

    private fun buildContent(
        session: SseStreamSession,
        isSuccess: Boolean,
        errorMsg: String?
    ): String {
        if (!isSuccess) {
            return errorMsg ?: "网络连接发生异常"
        }
        val cleanReply = cleanText(SseSessionContent.fullText(session))
        val singleLine = cleanReply.replace("\n", " ").replace("\r", " ").trim()
        return if (singleLine.isNotEmpty()) {
            if (singleLine.length > 80) singleLine.take(80) + "..." else singleLine
        } else {
            "回复内容已生成，点击进入应用查看。"
        }
    }

    private fun buildPendingIntent(session: SseStreamSession): PendingIntent {
        val openIntent = buildOpenIntent(session)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                PendingIntent.FLAG_IMMUTABLE
            } else {
                0
            }
        return PendingIntent.getActivity(
            context,
            notificationId(session),
            openIntent,
            flags
        )
    }

    private fun buildOpenIntent(session: SseStreamSession): Intent {
        return try {
            val activityClass = Class.forName("com.vcp.avatar.MainActivity")
            Intent(context, activityClass).apply {
                flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
                putExtra("requestId", session.key.messageId)
                putExtra("topicId", session.key.topicId)
                putExtra("ownerId", session.key.ownerId)
                putExtra("ownerType", session.key.ownerType)
            }
        } catch (_: ClassNotFoundException) {
            Intent(Intent.ACTION_MAIN).apply {
                setPackage(context.packageName)
                addCategory(Intent.CATEGORY_LAUNCHER)
            }
        }
    }

    private fun notificationId(session: SseStreamSession): Int {
        return session.key.stableToken().hashCode()
    }

    private fun cleanText(text: String): String {
        var clean = text
        clean = clean.replace(
            Regex("\\[--- VCP元思考链:[\\s\\S]*?元思考链结束 ---\\]", RegexOption.IGNORE_CASE),
            ""
        )
        clean = clean.replace(Regex("<think>[\\s\\S]*?</think>", RegexOption.IGNORE_CASE), "")
        clean = clean.replace(Regex("<think>[\\s\\S]*", RegexOption.IGNORE_CASE), "")
        clean = clean.replace(
            Regex("\\[--- VCP元思考链:[\\s\\S]*", RegexOption.IGNORE_CASE),
            ""
        )
        clean = clean.replace(Regex("\\n\\s*\\n+"), "\n")
        return clean.trim()
    }
}
