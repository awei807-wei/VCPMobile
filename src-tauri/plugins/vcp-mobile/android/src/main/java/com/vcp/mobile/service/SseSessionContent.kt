package com.vcp.mobile.service

import android.util.Log
import org.json.JSONObject

/** 从缓存的 OpenAI 兼容 SSE 事件中提取助手正文。 */
object SseSessionContent {
    private const val TAG = "VcpSseContent"

    fun fullText(session: SseStreamSession): String = synchronized(session) {
        fullText(session.eventBuffer)
    }

    fun fullText(events: List<JSONObject>): String {
        val text = StringBuilder()
        for (event in events) {
            if (event.optString("eventType") != "message") {
                continue
            }
            val eventData = event.optString("eventData")
            if (eventData == "[DONE]") {
                continue
            }
            appendDelta(text, eventData)
        }
        return text.toString()
    }

    private fun appendDelta(text: StringBuilder, eventData: String) {
        try {
            val data = JSONObject(eventData)
            val choices = data.optJSONArray("choices") ?: return
            if (choices.length() == 0) {
                return
            }
            val delta = choices.optJSONObject(0)?.optJSONObject("delta") ?: return
            val content = delta.opt("content")
            if (content != null && content !== JSONObject.NULL) {
                text.append(content.toString())
            }
        } catch (error: Exception) {
            // 非 JSON SSE 消息可能是合法供应商数据，但不贡献正文；记录长度便于排查。
            Log.d(TAG, "忽略无法解析的 SSE 事件：result=ignored, length=${eventData.length}")
        }
    }
}

/** 客户端断开后，为已完成流写入持久化恢复文件。 */
object SseSessionPersistence {
    private const val TAG = "VcpSsePersistence"

    fun dump(context: android.content.Context, session: SseStreamSession): Boolean {
        return try {
            val cacheDir = java.io.File(context.cacheDir, "sse_cache")
            if (!cacheDir.exists() && !cacheDir.mkdirs() && !cacheDir.exists()) {
                Log.e(TAG, "创建恢复目录失败：result=error")
                return false
            }
            val file = java.io.File(cacheDir, "sse_recovered_${session.key.stableToken()}.json")
            val temporary = java.io.File(cacheDir, "${file.name}.${session.generation}.tmp")
            val dump = buildDump(session)
            temporary.writeText(dump.toString())
            if (!temporary.renameTo(file)) {
                temporary.delete()
                Log.e(TAG, "恢复文件替换失败：result=error")
                false
            } else {
                true
            }
        } catch (error: Exception) {
            Log.e(TAG, "恢复文件写入失败：result=error")
            false
        }
    }

    internal fun buildDump(session: SseStreamSession): JSONObject {
        val fields = buildDumpFields(session)
        return JSONObject().apply {
            fields.forEach { (name, value) -> put(name, value) }
        }
    }

    internal fun buildDumpFields(session: SseStreamSession): Map<String, Any> {
        require(session.generation > 0L) {
            "恢复文件缺少有效 helper generation"
        }
        return mapOf(
            "content" to SseSessionContent.fullText(session),
            "finishReason" to (session.lastFinishReason ?: "completed"),
            "timestamp" to System.currentTimeMillis(),
            "generation" to session.generation
        )
    }
}
