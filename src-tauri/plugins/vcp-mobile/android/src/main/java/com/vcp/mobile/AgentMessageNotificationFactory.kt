package com.vcp.mobile

import android.app.Activity
import android.app.Notification
import android.app.PendingIntent
import android.content.Intent
import android.media.RingtoneManager
import androidx.core.app.NotificationCompat

/** 构造可由 Android 通知栏统一展开、收起的 Agent 消息通知组。 */
internal class AgentMessageNotificationFactory(
    private val activity: Activity,
    private val channelId: String,
    private val groupKey: String
) {
    fun buildMessage(title: String, body: String, now: Long): Notification {
        val builder = NotificationCompat.Builder(activity, channelId)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setGroup(groupKey)
            .setGroupAlertBehavior(NotificationCompat.GROUP_ALERT_SUMMARY)
            .setAutoCancel(true)
            .setWhen(now)
            .setShowWhen(true)
        buildLaunchPendingIntent()?.let(builder::setContentIntent)
        return builder.build()
    }

    fun buildSummary(
        latestTitle: String,
        latestBody: String,
        groupedCount: Int,
        now: Long
    ): Notification {
        val bodyPreview = latestBody
            .replace("\n", " ")
            .replace("\r", " ")
            .trim()
        val latestPreview = if (groupedCount > 1) {
            "$latestTitle：$bodyPreview"
        } else {
            bodyPreview
        }.take(180)
        val builder = NotificationCompat.Builder(activity, channelId)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(if (groupedCount > 1) "$groupedCount 条 Agent 消息" else latestTitle)
            .setContentText(latestPreview)
            .setStyle(
                NotificationCompat.InboxStyle()
                    .addLine(latestPreview)
                    .setSummaryText(if (groupedCount > 1) "已自动收起，展开查看详情" else "Agent 消息")
            )
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setDefaults(NotificationCompat.DEFAULT_SOUND or NotificationCompat.DEFAULT_VIBRATE)
            .setSound(RingtoneManager.getDefaultUri(RingtoneManager.TYPE_NOTIFICATION))
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setGroup(groupKey)
            .setGroupSummary(true)
            .setGroupAlertBehavior(NotificationCompat.GROUP_ALERT_SUMMARY)
            .setNumber(groupedCount)
            .setOnlyAlertOnce(true)
            .setAutoCancel(true)
            .setWhen(now)
            .setShowWhen(true)
        buildLaunchPendingIntent()?.let(builder::setContentIntent)
        return builder.build()
    }

    private fun buildLaunchPendingIntent(): PendingIntent? {
        val launchIntent = activity.packageManager
            .getLaunchIntentForPackage(activity.packageName)
            ?.apply {
                addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            } ?: return null
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        return PendingIntent.getActivity(activity, 0, launchIntent, flags)
    }
}
