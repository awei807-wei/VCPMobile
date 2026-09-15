package com.vcp.mobile

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentNotificationBurstGateTest {
    @Test
    fun `首条消息开启一波通知`() {
        val decision = AgentNotificationBurstGate().evaluate("first", 1_000)

        assertFalse(decision.skipDuplicate)
        assertTrue(decision.startsNewBurst)
    }

    @Test
    fun `短时间连续消息归入同一波通知`() {
        val gate = AgentNotificationBurstGate(burstWindowMs = 2_000)

        gate.evaluate("first", 1_000)
        val second = gate.evaluate("second", 1_500)
        val third = gate.evaluate("third", 3_000)

        assertFalse(second.skipDuplicate)
        assertFalse(second.startsNewBurst)
        assertFalse(third.startsNewBurst)
    }

    @Test
    fun `静默窗口结束后的消息重新触发提醒`() {
        val gate = AgentNotificationBurstGate(burstWindowMs = 2_000)

        gate.evaluate("first", 1_000)
        val decision = gate.evaluate("second", 3_000)

        assertFalse(decision.skipDuplicate)
        assertTrue(decision.startsNewBurst)
    }

    @Test
    fun `五秒内相同消息被抑制且不会延长当前波次`() {
        val gate = AgentNotificationBurstGate(
            duplicateWindowMs = 5_000,
            burstWindowMs = 2_000
        )

        gate.evaluate("same", 1_000)
        val duplicate = gate.evaluate("same", 4_000)
        val next = gate.evaluate("next", 4_100)

        assertTrue(duplicate.skipDuplicate)
        assertFalse(duplicate.startsNewBurst)
        assertTrue(next.startsNewBurst)
    }

    @Test
    fun `系统时钟回拨时安全开启新波次`() {
        val gate = AgentNotificationBurstGate()

        gate.evaluate("first", 5_000)
        val decision = gate.evaluate("second", 4_000)

        assertFalse(decision.skipDuplicate)
        assertTrue(decision.startsNewBurst)
    }
}
