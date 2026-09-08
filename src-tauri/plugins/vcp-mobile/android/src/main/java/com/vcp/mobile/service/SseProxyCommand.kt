package com.vcp.mobile.service

import org.json.JSONObject

/** 解析 helper 长度前缀 socket 收到的命令。 */
data class SseProxyCommand(
    val action: String,
    val key: StreamSessionKey,
    val url: String? = null,
    val headersJson: String = "{}",
    val body: String = "",
    val contextJson: JSONObject? = null,
    val startIndex: Int = 0,
    val expectedGeneration: Long? = null,
    val requestEpoch: Long? = null,
) {
    companion object {
        private val supportedActions = setOf(
            "start",
            "query",
            "prepare_resume",
            "resume",
            "cancel_resume",
            "stop",
        )

        fun parse(request: JSONObject): SseProxyCommand {
            val context = request.optJSONObject("context")
            val action = request.getString("action")
            val key = parseKey(
                action,
                identityFields(request),
                context?.let(::identityFields)
            )
            val generationText = request.optString(
                "expectedGeneration",
                request.optString("generation", "")
            )
            val generation = if (generationText.isBlank()) {
                null
            } else {
                generationText.toLongOrNull()
                    ?.takeIf { it > 0 }
                    ?: throw IllegalArgumentException("expectedGeneration 无效")
            }
            requireExpectedGeneration(action, generation)
            val requestEpoch = parsePositiveLong(request.opt("requestEpoch"), "requestEpoch")
            requireRequestEpoch(action, requestEpoch)

            return SseProxyCommand(
                action = action,
                key = key,
                url = request.optString("url", "").takeIf(String::isNotBlank),
                headersJson = request.optString("headers", "{}"),
                body = request.optString("body", ""),
                contextJson = context,
                startIndex = request.optInt("startIndex", 0).coerceAtLeast(0),
                expectedGeneration = generation,
                requestEpoch = requestEpoch,
            )
        }

        private fun parseKey(
            action: String,
            fields: SseProxyIdentityFields,
            contextFields: SseProxyIdentityFields?
        ): StreamSessionKey {
            if (action !in supportedActions) {
                throw IllegalArgumentException("不支持的 helper action：$action")
            }
            val key = parseCompleteKey(fields, "命令")
            val contextKey = contextFields?.let { parseContextKey(it) }
            if (contextKey != null && contextKey != key) {
                throw IllegalArgumentException("命令与 context 的流身份不一致")
            }
            return key
        }

        private fun parseContextKey(fields: SseProxyIdentityFields): StreamSessionKey? {
            if (!fields.hasAny) {
                return null
            }
            return parseCompleteKey(fields, "context")
        }

        private fun parseCompleteKey(fields: SseProxyIdentityFields, source: String): StreamSessionKey {
            val messageId = fields.messageId
            val requestId = fields.requestId
            if (messageId != null && requestId != null && messageId != requestId) {
                throw IllegalArgumentException("$source 的 requestId 与 messageId 不一致")
            }
            val ownerType = fields.ownerType
                .requireIdentity("$source.ownerType")
            if (ownerType != "agent" && ownerType != "group") {
                throw IllegalArgumentException("$source.ownerType 不受支持")
            }
            return StreamSessionKey(
                ownerType = ownerType,
                ownerId = fields.ownerId.requireIdentity("$source.ownerId"),
                topicId = fields.topicId.requireIdentity("$source.topicId"),
                messageId = (messageId ?: requestId).requireIdentity("$source.messageId/requestId")
            )
        }

        internal fun parseIdentity(
            action: String,
            fields: SseProxyIdentityFields,
            contextFields: SseProxyIdentityFields? = null
        ): StreamSessionKey {
            return parseKey(action, fields, contextFields)
        }

        /** 连接接管和 stop 必须携带当前 helper generation；start/query 可以不带。 */
        internal fun requireExpectedGeneration(action: String, generation: Long?): Long? {
            if (action == "stop" || action == "prepare_resume" ||
                action == "resume" || action == "cancel_resume"
            ) {
                return generation
                    ?.takeIf { it > 0 }
                    ?: throw IllegalArgumentException("$action 命令必须提供正整数 expectedGeneration")
            }
            return generation
        }

        /** start 必须携带 Rust registry 授权的正整数请求纪元。 */
        internal fun requireRequestEpoch(action: String, requestEpoch: Long?): Long? {
            if (action == "start") {
                return requestEpoch
                    ?.takeIf { it > 0 }
                    ?: throw IllegalArgumentException("start 命令必须提供正整数 requestEpoch")
            }
            return requestEpoch
        }

        private fun parsePositiveLong(value: Any?, name: String): Long? {
            if (value == null || value === JSONObject.NULL) return null
            val parsed = when (value) {
                is Byte, is Short, is Int, is Long -> (value as Number).toLong()
                is Number -> value.toString().toLongOrNull()
                else -> value.toString().takeIf(String::isNotBlank)?.toLongOrNull()
            }
            return parsed?.takeIf { it > 0 }
                ?: throw IllegalArgumentException("$name 无效")
        }

        private fun identityFields(value: JSONObject): SseProxyIdentityFields {
            return SseProxyIdentityFields(
                ownerType = stringValue(value, "ownerType"),
                ownerId = stringValue(value, "ownerId"),
                topicId = stringValue(value, "topicId"),
                messageId = stringValue(value, "messageId"),
                requestId = stringValue(value, "requestId"),
                hasIdentityField = listOf(
                    "ownerType",
                    "ownerId",
                    "topicId",
                    "messageId",
                    "requestId"
                ).any { value.has(it) && value.opt(it) !== JSONObject.NULL }
            )
        }

        private fun stringValue(json: JSONObject?, name: String): String? {
            val value = json?.opt(name) ?: return null
            if (value === JSONObject.NULL) {
                return null
            }
            return value.toString()
        }

        private fun String?.requireIdentity(name: String): String {
            return this?.takeIf(String::isNotBlank)
                ?: throw IllegalArgumentException("缺少流身份字段：$name")
        }
    }
}

/** 从 socket 命令或 context 提取出的身份字段，供纯 JVM 规则测试复用。 */
data class SseProxyIdentityFields(
    val ownerType: String? = null,
    val ownerId: String? = null,
    val topicId: String? = null,
    val messageId: String? = null,
    val requestId: String? = null,
    private val hasIdentityField: Boolean = false
) {
    val hasAny: Boolean
        get() = hasIdentityField || listOf(ownerType, ownerId, topicId, messageId, requestId)
            .any { it != null }
}
