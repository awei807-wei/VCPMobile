package com.vcp.mobile.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.security.SecureRandom

class SseProxyAuthTest {
    @Test
    fun `每个 helper token 为 256 bit 且实例间不同`() {
        val first = SseProxyAuth.generateToken(SecureRandom())
        val second = SseProxyAuth.generateToken(SecureRandom())

        assertTrue(first.matches(Regex("[A-Za-z0-9_-]{43}")))
        assertNotEquals(first, second)
        assertTrue(SseProxyAuth.constantTimeEquals(first, first))
        assertFalse(SseProxyAuth.constantTimeEquals(first, second))
        assertFalse(SseProxyAuth.constantTimeEquals(first, null))
    }
}
