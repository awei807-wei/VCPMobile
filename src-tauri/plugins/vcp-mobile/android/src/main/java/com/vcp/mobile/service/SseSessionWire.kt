package com.vcp.mobile.service

import android.util.Log
import org.json.JSONObject
import java.io.OutputStream

/** helper socket 的握手和错误响应帧。 */
object SseSessionWire {
    private const val TAG = "VcpSseSessions"

    fun writeGenerationAck(output: OutputStream, session: SseStreamSession, generation: Long) {
        val response = JSONObject().apply {
            put("requestId", session.requestId)
            put("messageId", session.requestId)
            put("ownerType", session.key.ownerType)
            put("ownerId", session.key.ownerId)
            put("topicId", session.key.topicId)
            put("eventType", "started")
            put("generation", generation)
            put("index", -1)
        }
        writeResponseOrThrow(
            output,
            response,
            "{\"requestId\":${quote(session.requestId)}," +
                "\"messageId\":${quote(session.requestId)}," +
                "\"ownerType\":${quote(session.key.ownerType)}," +
                "\"ownerId\":${quote(session.key.ownerId)}," +
                "\"topicId\":${quote(session.key.topicId)}," +
                "\"eventType\":\"started\",\"generation\":$generation,\"index\":-1}",
        )
    }

    /** Confirm that the helper has reserved a same-generation socket handoff. */
    fun writeTakeoverAck(output: OutputStream, session: SseStreamSession, generation: Long) {
        val response = JSONObject().apply {
            put("requestId", session.requestId)
            put("messageId", session.requestId)
            put("ownerType", session.key.ownerType)
            put("ownerId", session.key.ownerId)
            put("topicId", session.key.topicId)
            put("eventType", "resume_pending")
            put("generation", generation)
            put("index", -1)
        }
        writeResponseOrThrow(
            output,
            response,
            "{\"requestId\":${quote(session.requestId)}," +
                "\"messageId\":${quote(session.requestId)}," +
                "\"ownerType\":${quote(session.key.ownerType)}," +
                "\"ownerId\":${quote(session.key.ownerId)}," +
                "\"topicId\":${quote(session.key.topicId)}," +
                "\"eventType\":\"resume_pending\",\"generation\":$generation," +
                "\"index\":-1}",
        )
    }

    fun writeMissingSession(output: OutputStream, key: StreamSessionKey) {
        val response = JSONObject().apply {
            put("requestId", key.messageId)
            put("messageId", key.messageId)
            put("ownerType", key.ownerType)
            put("ownerId", key.ownerId)
            put("topicId", key.topicId)
            put("eventType", "error")
            put("generation", JSONObject.NULL)
            put("eventData", "{\"error\":\"找不到会话\"}")
        }
        writeResponse(
            output,
            response,
            "{\"requestId\":${quote(key.messageId)}," +
                "\"messageId\":${quote(key.messageId)}," +
                "\"ownerType\":${quote(key.ownerType)}," +
                "\"ownerId\":${quote(key.ownerId)}," +
                "\"topicId\":${quote(key.topicId)}," +
                "\"eventType\":\"error\",\"generation\":null," +
                "\"eventData\":{\"error\":\"找不到会话\"}}",
        )
    }

    fun writeResponse(output: OutputStream, response: String) {
        try {
            writeResponseOrThrow(output, response)
        } catch (error: Exception) {
            Log.e(TAG, "写入 helper 响应失败：result=error")
        }
    }

    fun writeResponse(output: OutputStream, response: JSONObject, fallback: String) {
        val encoded: String? = response.toString()
        writeResponse(output, encoded?.takeIf(String::isNotEmpty) ?: fallback)
    }

    /**
     * 写入必须送达的握手或回放帧，并保留底层异常给会话所有者处理。
     * 普通查询/错误响应仍使用 [writeResponse] 的 best-effort 语义。
     */
    private fun writeResponseOrThrow(
        output: OutputStream,
        response: JSONObject,
        fallback: String,
    ) {
        val encoded: String? = response.toString()
        SseSocketCodec.write(output, encoded?.takeIf(String::isNotEmpty) ?: fallback)
    }

    private fun writeResponseOrThrow(output: OutputStream, response: String) {
        SseSocketCodec.write(output, response)
    }

    fun writeQueryResponse(
        output: OutputStream,
        key: StreamSessionKey,
        status: String,
        generation: Long?,
        lastFinishReason: String,
        lastEventIndex: Int,
        content: String,
    ) {
        val response = JSONObject().apply {
            put("requestId", key.messageId)
            put("messageId", key.messageId)
            put("ownerType", key.ownerType)
            put("ownerId", key.ownerId)
            put("topicId", key.topicId)
            put("status", status)
            put("generation", generation ?: JSONObject.NULL)
            put("lastFinishReason", lastFinishReason)
            put("lastEventIndex", lastEventIndex)
            put("content", content)
        }
        val generationJson = generation?.let { ",\"generation\":$it" } ?: ",\"generation\":null"
        val fallback = "{\"requestId\":${quote(key.messageId)}," +
            "\"messageId\":${quote(key.messageId)}," +
            "\"ownerType\":${quote(key.ownerType)}," +
            "\"ownerId\":${quote(key.ownerId)}," +
            "\"topicId\":${quote(key.topicId)}," +
            "\"status\":${quote(status)}$generationJson," +
            "\"lastFinishReason\":${quote(lastFinishReason)}," +
            "\"lastEventIndex\":$lastEventIndex," +
            "\"content\":${quote(content)}}"
        writeResponse(output, response, fallback)
    }

    private fun quote(value: String): String {
        val encoded: String? = JSONObject.quote(value)
        if (!encoded.isNullOrEmpty()) return encoded
        val escaped = buildString {
            value.forEach { character ->
                when (character) {
                    '\\' -> append("\\\\")
                    '"' -> append("\\\"")
                    '\n' -> append("\\n")
                    '\r' -> append("\\r")
                    '\t' -> append("\\t")
                    else -> append(character)
                }
            }
        }
        return "\"$escaped\""
    }
}
