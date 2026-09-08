package com.vcp.mobile.service

import android.util.Log
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject

/** 构造供应商 SSE 请求，并解析 helper 传来的请求头。 */
object SseSessionRequest {
    private const val TAG = "VcpSseRequest"

    fun build(command: SseProxyCommand, url: String): Request {
        val builder = Request.Builder().url(url)
        parseHeaders(command.headersJson).forEach { (name, value) -> builder.header(name, value) }
        if (command.body.isNotEmpty()) {
            builder.post(command.body.toRequestBody("application/json; charset=utf-8".toMediaType()))
        }
        return builder.build()
    }

    private fun parseHeaders(headersJson: String): Map<String, String> {
        return try {
            val headers = JSONObject(headersJson)
            val values = LinkedHashMap<String, String>()
            val keys = headers.keys()
            while (keys.hasNext()) {
                val key = keys.next()
                values[key] = headers.getString(key)
            }
            values
        } catch (error: Exception) {
            Log.e(TAG, "解析 SSE 请求头失败：result=error")
            emptyMap()
        }
    }
}
