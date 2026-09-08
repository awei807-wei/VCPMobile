package com.vcp.mobile.service

/** 根据消费者命名空间选择默认超时，避免守护者主类继续膨胀。 */
internal object ForegroundTimeoutPolicy {
    fun forTag(tag: String): Long {
        return when {
            tag.startsWith("stream:") -> 10 * 60 * 1000L
            tag == "sync" || tag == "prerender" -> 30 * 60 * 1000L
            ForegroundTagContract.isDistributedTag(tag) || tag == "manual_keepalive" ->
                2 * 60 * 60 * 1000L
            else -> 15 * 60 * 1000L
        }
    }
}
