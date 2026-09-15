package com.vcp.mobile

import org.junit.Assert.assertEquals
import org.junit.Test

class AgentMessageNotificationGroupResetTest {
    @Test
    fun `新波次只清理旧 Agent 消息分组和摘要`() {
        val groupKey = "com.vcp.mobile.AGENT_MESSAGES"
        val summaryId = -0x41474D
        val groupedChildId = 101
        val unrelatedNotificationId = 202

        val idsToCancel = agentMessageNotificationIdsToCancel(
            activeNotifications = listOf(
                ActiveNotificationGroupEntry(groupedChildId, groupKey),
                ActiveNotificationGroupEntry(summaryId, null),
                ActiveNotificationGroupEntry(unrelatedNotificationId, "com.vcp.mobile.OTHER"),
            ),
            groupKey = groupKey,
            summaryId = summaryId,
        )

        assertEquals(setOf(groupedChildId, summaryId), idsToCancel)
    }
}
