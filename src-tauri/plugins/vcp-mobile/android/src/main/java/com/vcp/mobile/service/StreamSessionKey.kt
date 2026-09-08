package com.vcp.mobile.service

import java.security.MessageDigest

/** 一个 helper 流的完整身份。 */
data class StreamSessionKey(
    val ownerType: String,
    val ownerId: String,
    val topicId: String,
    val messageId: String
) {
    init {
        require(ownerType.isNotBlank()) { "ownerType 不能为空" }
        require(ownerId.isNotBlank()) { "ownerId 不能为空" }
        require(topicId.isNotBlank()) { "topicId 不能为空" }
        require(messageId.isNotBlank()) { "messageId 不能为空" }
    }

    /** 使用 UTF-8 字节长度前缀，避免跨语言或分隔符碰撞。 */
    fun canonicalValue(): String = listOf(ownerType, ownerId, topicId, messageId)
        .joinToString("") {
            val bytes = it.toByteArray(Charsets.UTF_8)
            "${bytes.size}:$it"
        }

    /** 生成适合文件名和通知 ID 的稳定身份 token。 */
    fun stableToken(): String {
        val digest = MessageDigest.getInstance("SHA-256")
        return digest.digest(canonicalValue().toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it.toInt() and 0xff) }
    }

    override fun toString(): String {
        return "StreamSessionKey(ownerType=$ownerType, ownerId=$ownerId, " +
            "topicId=$topicId, messageId=$messageId)"
    }
}

/** 流式前台消费者共用的稳定 tag 命名空间。 */
object StreamLeaseTag {
    fun forKey(key: StreamSessionKey): String {
        return listOf(key.ownerType, key.ownerId, key.topicId, key.messageId)
            .joinToString(":") { value ->
                value.toByteArray(Charsets.UTF_8)
                    .joinToString("") { byte -> "%02x".format(byte.toInt() and 0xff) }
            }
            .let { encoded -> "stream:session:$encoded" }
    }

    fun forLegacyAgent(agentName: String): String {
        val digest = MessageDigest.getInstance("SHA-256")
        val token = digest.digest(agentName.toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it.toInt() and 0xff) }
        return "stream:legacy:$token"
    }
}
