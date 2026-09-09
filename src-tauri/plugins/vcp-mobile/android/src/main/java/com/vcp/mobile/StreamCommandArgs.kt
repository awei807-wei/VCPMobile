package com.vcp.mobile

import app.tauri.annotation.InvokeArg
import com.vcp.mobile.service.StreamSessionKey

/** 流式前台命令的参数与完整身份解析。 */
@InvokeArg
class StartStreamArgs {
    lateinit var agentName: String
    var isKeepaliveMode: Boolean? = null
    var identity: StreamIdentityArgs? = null
    var ownerType: String? = null
    var ownerId: String? = null
    var topicId: String? = null
    var messageId: String? = null
    var expectedGeneration: Long? = null
}

@InvokeArg
class StopStreamArgs {
    var identity: StreamIdentityArgs? = null
    var ownerType: String? = null
    var ownerId: String? = null
    var topicId: String? = null
    var messageId: String? = null
    var expectedGeneration: Long? = null
}

@InvokeArg
class StreamIdentityArgs {
    var ownerType: String? = null
    var ownerId: String? = null
    var topicId: String? = null
    var messageId: String? = null
}

internal fun StartStreamArgs.streamSessionKeyOrNull(): StreamSessionKey? {
    return streamSessionKeyOrNull(
        chooseIdentity(identity?.ownerType, ownerType, "ownerType"),
        chooseIdentity(identity?.ownerId, ownerId, "ownerId"),
        chooseIdentity(identity?.topicId, topicId, "topicId"),
        chooseIdentity(identity?.messageId, messageId, "messageId")
    )
}

internal fun StopStreamArgs.streamSessionKeyOrNull(): StreamSessionKey? {
    return streamSessionKeyOrNull(
        chooseIdentity(identity?.ownerType, ownerType, "ownerType"),
        chooseIdentity(identity?.ownerId, ownerId, "ownerId"),
        chooseIdentity(identity?.topicId, topicId, "topicId"),
        chooseIdentity(identity?.messageId, messageId, "messageId")
    )
}

private fun chooseIdentity(primary: String?, fallback: String?, name: String): String? {
    if (primary != null && fallback != null && primary != fallback) {
        throw IllegalArgumentException("流身份字段 $name 重复且不一致")
    }
    return primary?.takeIf(String::isNotBlank) ?: fallback?.takeIf(String::isNotBlank)
}

internal fun streamSessionKeyOrNull(
    ownerType: String?,
    ownerId: String?,
    topicId: String?,
    messageId: String?
): StreamSessionKey? {
    if (ownerType.isNullOrBlank() || ownerId.isNullOrBlank() ||
        topicId.isNullOrBlank() || messageId.isNullOrBlank()
    ) {
        return null
    }
    return StreamSessionKey(ownerType, ownerId, topicId, messageId)
}

internal fun Long?.requirePositiveGeneration(action: String): Long {
    return this?.takeIf { it > 0 }
        ?: throw IllegalArgumentException("$action 命令必须提供正整数 expectedGeneration")
}
