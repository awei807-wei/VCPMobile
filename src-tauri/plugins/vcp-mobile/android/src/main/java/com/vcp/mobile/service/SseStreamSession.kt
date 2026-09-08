package com.vcp.mobile.service

import okhttp3.sse.EventSource
import org.json.JSONObject
import java.io.OutputStream

/** 一个 helper 流 generation 独占的可变状态。 */
class SseStreamSession(
    val key: StreamSessionKey,
    val contextJson: JSONObject?,
    @Volatile var eventSource: EventSource? = null,
    @Volatile var isCompleted: Boolean = false,
    @Volatile var lastFinishReason: String? = null,
    /** Rust registry 的 authoritative request epoch；测试/恢复构造默认无 epoch。 */
    val requestEpoch: Long = 0L,
) {
    val requestId: String
        get() = key.messageId

    val eventBuffer: MutableList<JSONObject> = mutableListOf()

    @Volatile
    var activeSocketOutputStream: OutputStream? = null

    @Volatile
    var dumpedToDisk: Boolean = false

    @Volatile
    var cleanupScheduled: Boolean = false

    /** 恢复文件写入或等待重试期间保持 helper 引用，避免提前自停。 */
    internal val dumpRetryLedger: SseDumpRetryLedger = SseDumpRetryLedger()

    @Volatile
    var socketGeneration: Long = 0L

    @Volatile
    var generation: Long = 0L

    /**
     * 同一 helper generation 的 socket 接管门闩。
     *
     * Rust 会先登记 resume，再让旧 socket 退出；在新 socket 的 ACK/replay
     * 提交前，旧 EOF 只能释放它自己的连接，不能删除整个 helper session。
     */
    @Volatile
    var pendingTakeoverToken: Long? = null

    @Volatile
    var pendingTakeoverConnectionGeneration: Long? = null

    /** 旧 socket 在 pending takeover 期间已断开，且尚未提交候选连接。 */
    @Volatile
    var pendingTakeoverConnectionLost: Boolean = false
}
