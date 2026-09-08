package com.vcp.mobile.service

/** 只向日志提供固定类别，避免把原始 lease tag 写入 release 日志。 */
internal fun categoryForPriority(priority: Int): String {
    return when (priority) {
        ForegroundGuardian.PRIORITY_SYNC -> "sync"
        ForegroundGuardian.PRIORITY_PRERENDER -> "prerender"
        ForegroundGuardian.PRIORITY_STREAM -> "stream"
        ForegroundGuardian.PRIORITY_DISTRIBUTED -> "distributed"
        else -> "other"
    }
}

internal fun categoryForTag(tag: String): String {
    return when {
        ForegroundTagContract.isDistributedTag(tag) -> "distributed"
        tag.startsWith("stream:session:") || tag.startsWith("stream:") -> "stream"
        else -> "other"
    }
}

internal fun isIdentityStreamTag(tag: String): Boolean {
    return tag.startsWith("stream:session:")
}
