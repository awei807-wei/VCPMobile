package com.vcp.mobile.service

/** Rust 与 Android 共用的分布式前台消费者命名空间。 */
internal object ForegroundTagContract {
    const val DISTRIBUTED_TAG = "distributed"
    private const val DISTRIBUTED_PREFIX = "$DISTRIBUTED_TAG:"
    private const val GENERATION_MARKER = ":generation:"

    /** 兼容旧恢复占位 tag，并覆盖所有带 generation 的分布式消费者。 */
    fun isDistributedTag(tag: String): Boolean {
        return tag == DISTRIBUTED_TAG || tag.startsWith(DISTRIBUTED_PREFIX)
    }

    fun isGenerationDistributedTag(tag: String): Boolean {
        return tag.startsWith(DISTRIBUTED_PREFIX) && tag.contains(GENERATION_MARKER)
    }
}
