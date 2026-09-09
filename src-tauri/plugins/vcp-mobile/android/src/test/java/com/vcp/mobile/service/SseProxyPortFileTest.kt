package com.vcp.mobile.service

import android.content.Context
import android.content.ContextWrapper
import java.io.File
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class SseProxyPortFileTest {
    @Test
    fun `endpoint 只接受 version 1 JSON`() {
        val endpoint = SseProxyPortFile.parse(
            "{\"version\":1,\"port\":38417,\"token\":\"instance-token\"}"
        )
        assertEquals(38417, endpoint.port)
        assertEquals("instance-token", endpoint.token)
        assertRejects {
            SseProxyPortFile.parse("38417")
        }
        assertRejects {
            SseProxyPortFile.parse("{\"version\":2,\"port\":38417,\"token\":\"x\"}")
        }
        assertRejects {
            SseProxyPortFile.parse("{\"version\":1,\"port\":38417}")
        }
    }

    @Test
    fun `并发发布使用独立临时文件且最终端点完整`() {
        val directory = Files.createTempDirectory("sse-port-test").toFile()
        try {
            val context = CacheContext(directory)
            val start = CountDownLatch(1)
            val executor = Executors.newFixedThreadPool(4)
            val tokens = (0 until 4).map { index -> "token-$index" }
            val futures = tokens.mapIndexed { index, token ->
                executor.submit {
                    assertTrue(start.await(5, TimeUnit.SECONDS))
                    assertTrue(SseProxyPortFile.write(context, 38_417 + index, token))
                }
            }
            start.countDown()
            futures.forEach { it.get(5, TimeUnit.SECONDS) }
            executor.shutdownNow()

            val endpoint = SseProxyPortFile.read(context)
            assertTrue(endpoint != null)
            assertTrue(tokens.contains(endpoint!!.token))
            assertFalse(
                directory.listFiles()?.any { it.name.startsWith("sse_helper.port.") } == true,
            )
        } finally {
            directory.deleteRecursively()
        }
    }

    @Test
    fun `旧 token 不得删除新实例端点`() {
        val directory = Files.createTempDirectory("sse-port-test").toFile()
        try {
            val context = CacheContext(directory)
            assertTrue(SseProxyPortFile.write(context, 38_417, "old-token"))
            assertTrue(SseProxyPortFile.write(context, 38_418, "new-token"))
            assertFalse(SseProxyPortFile.deleteIfMatches(context, 38_417, "old-token"))
            assertEquals(38_418, SseProxyPortFile.read(context)?.port)
            assertEquals("new-token", SseProxyPortFile.read(context)?.token)
            assertTrue(SseProxyPortFile.deleteIfMatches(context, 38_418, "new-token"))
            assertEquals(null, SseProxyPortFile.read(context))
        } finally {
            directory.deleteRecursively()
        }
    }

    private class CacheContext(private val directory: File) : ContextWrapper(null) {
        override fun getApplicationContext(): Context = this
        override fun getCacheDir(): File = directory
    }

    private fun assertRejects(action: () -> Unit) {
        try {
            action()
            fail("旧格式或无效 endpoint 必须被拒绝")
        } catch (_: IllegalArgumentException) {
            // 预期拒绝。
        }
    }
}
