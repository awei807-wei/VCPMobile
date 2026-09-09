package com.vcp.mobile.service

import org.junit.Assert.assertEquals
import org.junit.Test

class SseSessionContentTest {
    @Test
    fun `恢复 dump 携带真实 helper generation`() {
        val session = sessionWithGeneration(17L)

        val dump = SseSessionPersistence.buildDumpFields(session)

        assertEquals(17L, dump["generation"])
    }

    @Test(expected = IllegalArgumentException::class)
    fun `没有 helper generation 的恢复 dump 被拒绝`() {
        SseSessionPersistence.buildDumpFields(sessionWithGeneration(0L))
    }

    private fun sessionWithGeneration(generation: Long): SseStreamSession {
        return SseStreamSession(
            StreamSessionKey("agent", "owner-1", "topic-1", "message-1"),
            null
        ).apply { this.generation = generation }
    }
}
