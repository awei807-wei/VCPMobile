package com.vcp.mobile.service

import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class SseProxyAuthorizationTest {
    @Test
    fun `缺失或错误 token 在 parser 和 dispatch 前被拒绝`() {
        val parserCalls = AtomicInteger(0)
        val parsed = SseProxyCommand(
            action = "query",
            key = StreamSessionKey("agent", "owner", "topic", "message"),
        )
        val parser: (org.json.JSONObject) -> SseProxyCommand = {
            parserCalls.incrementAndGet()
            parsed
        }

        listOf(null, "wrong-token").forEach { suppliedToken ->
            assertThrows(IllegalArgumentException::class.java) {
                authorizeHelperToken(
                    suppliedToken,
                    "instance-token",
                    org.json.JSONObject(),
                    parser,
                )
            }
        }
        assertEquals(0, parserCalls.get())
    }

    @Test
    fun `正确 token 才允许 parser 进入 dispatch 边界`() {
        val parserCalls = AtomicInteger(0)
        authorizeHelperToken(
            "instance-token",
            "instance-token",
            org.json.JSONObject(),
        ) { request ->
            parserCalls.incrementAndGet()
            SseProxyCommand(
                action = "query",
                key = StreamSessionKey("agent", "owner", "topic", "message"),
            )
        }
        assertEquals(1, parserCalls.get())
    }
}
