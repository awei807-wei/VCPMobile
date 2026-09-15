package com.vcp.mobile

internal data class AgentNotificationDecision(
    val skipDuplicate: Boolean,
    val startsNewBurst: Boolean
)

/**
 * 将短时间内连续到达的 Agent 消息视为同一波通知，同时保留原有的重复消息抑制。
 */
internal class AgentNotificationBurstGate(
    private val duplicateWindowMs: Long = 5_000,
    private val burstWindowMs: Long = 2_000
) {
    private var lastNotificationKey: String? = null
    private var lastAcceptedAt: Long? = null

    init {
        require(duplicateWindowMs > 0)
        require(burstWindowMs > 0)
    }

    @Synchronized
    fun evaluate(notificationKey: String, now: Long): AgentNotificationDecision {
        val previousAt = lastAcceptedAt
        val elapsed = previousAt?.let { now - it }
        val isForwardMovingClock = elapsed == null || elapsed >= 0
        val isDuplicate = notificationKey == lastNotificationKey &&
            elapsed != null &&
            isForwardMovingClock &&
            elapsed < duplicateWindowMs

        if (isDuplicate) {
            return AgentNotificationDecision(
                skipDuplicate = true,
                startsNewBurst = false
            )
        }

        val startsNewBurst = elapsed == null || !isForwardMovingClock || elapsed >= burstWindowMs
        lastNotificationKey = notificationKey
        lastAcceptedAt = now

        return AgentNotificationDecision(
            skipDuplicate = false,
            startsNewBurst = startsNewBurst
        )
    }
}
