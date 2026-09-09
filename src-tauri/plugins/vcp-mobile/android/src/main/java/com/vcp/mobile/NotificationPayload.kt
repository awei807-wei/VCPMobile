package com.vcp.mobile

import android.content.Intent
import app.tauri.plugin.JSObject

/** 通知点击传递到 WebView 的完整流身份。 */
data class NotificationPayload(
    val topicId: String,
    val ownerId: String,
    val ownerType: String,
    val requestId: String
)

object NotificationPayloadBridge {
    fun fromIntent(intent: Intent): NotificationPayload? {
        val topicId = intent.getStringExtra("topicId")?.takeIf(String::isNotBlank)
        val ownerId = intent.getStringExtra("ownerId")?.takeIf(String::isNotBlank)
        val ownerType = intent.getStringExtra("ownerType")?.takeIf(String::isNotBlank)
        val requestId = intent.getStringExtra("requestId")?.takeIf(String::isNotBlank)
        if (topicId == null || ownerId == null || ownerType == null || requestId == null) {
            return null
        }
        if (ownerType != "agent" && ownerType != "group") {
            return null
        }
        return NotificationPayload(topicId, ownerId, ownerType, requestId)
    }

    fun toJsObject(payload: NotificationPayload): JSObject = JSObject().apply {
        put("topicId", payload.topicId)
        put("ownerId", payload.ownerId)
        put("ownerType", payload.ownerType)
        put("requestId", payload.requestId)
    }

    fun consume(intent: Intent) {
        intent.removeExtra("topicId")
        intent.removeExtra("ownerId")
        intent.removeExtra("ownerType")
        intent.removeExtra("requestId")
    }
}
