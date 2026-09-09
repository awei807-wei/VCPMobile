package com.vcp.mobile.service

import java.nio.charset.StandardCharsets
import java.security.SecureRandom

/** helper loopback 命令的实例级鉴权，token 不进入日志或可观测对象。 */
internal object SseProxyAuth {
    private const val TOKEN_BYTES = 32

    fun generateToken(random: SecureRandom = SecureRandom()): String {
        val bytes = ByteArray(TOKEN_BYTES)
        random.nextBytes(bytes)
        return java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)
    }

    /** 使用固定上界循环比较，拒绝缺失 token 与长度差异。 */
    fun constantTimeEquals(expected: String, actual: String?): Boolean {
        if (actual == null) return false
        val expectedBytes = expected.toByteArray(StandardCharsets.UTF_8)
        val actualBytes = actual.toByteArray(StandardCharsets.UTF_8)
        var difference = expectedBytes.size xor actualBytes.size
        val length = maxOf(expectedBytes.size, actualBytes.size)
        for (index in 0 until length) {
            val expectedByte = if (index < expectedBytes.size) expectedBytes[index].toInt() else 0
            val actualByte = if (index < actualBytes.size) actualBytes[index].toInt() else 0
            difference = difference or (expectedByte xor actualByte)
        }
        return difference == 0
    }
}
