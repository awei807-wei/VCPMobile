package com.vcp.mobile.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ForegroundLockReleaseSchedulerTest {
    @Test
    fun `释放失败由调度器按固定退避自然重试并在成功后停止`() {
        val queued = mutableListOf<Pair<Runnable, Long>>()
        val errors = mutableListOf<String>()
        var releaseAttempts = 0
        val scheduler = ForegroundLockReleaseScheduler(
            postDelayed = { runnable, delay ->
                queued += runnable to delay
                true
            },
            removeCallbacks = { runnable ->
                queued.removeAll { it.first === runnable }
            },
            releaseLocks = {
                releaseAttempts += 1
                releaseAttempts < 3
            },
            logError = errors::add,
        )

        scheduler.release()
        assertEquals(listOf(100L), queued.map { it.second })
        queued.removeAt(0).first.run()
        assertEquals(listOf(250L), queued.map { it.second })
        queued.removeAt(0).first.run()

        assertEquals(3, releaseAttempts)
        assertTrue(queued.isEmpty())
        assertTrue(errors.isEmpty())
    }

    @Test
    fun `新 acquire 取消旧释放重试任务`() {
        val queued = mutableListOf<Pair<Runnable, Long>>()
        val scheduler = ForegroundLockReleaseScheduler(
            postDelayed = { runnable, delay ->
                queued += runnable to delay
                true
            },
            removeCallbacks = { runnable ->
                queued.removeAll { it.first === runnable }
            },
            releaseLocks = { true },
            logError = {},
        )

        scheduler.release()
        assertEquals(1, queued.size)
        scheduler.cancel()
        assertTrue(queued.isEmpty())
    }

    @Test
    fun `释放重试耗尽后记录可观测错误`() {
        val queued = mutableListOf<Pair<Runnable, Long>>()
        val errors = mutableListOf<String>()
        var releaseAttempts = 0
        val scheduler = ForegroundLockReleaseScheduler(
            postDelayed = { runnable, delay ->
                queued += runnable to delay
                true
            },
            removeCallbacks = { runnable ->
                queued.removeAll { it.first === runnable }
            },
            releaseLocks = {
                releaseAttempts += 1
                true
            },
            logError = errors::add,
        )

        scheduler.release()
        while (queued.isNotEmpty()) queued.removeAt(0).first.run()

        assertEquals(6, releaseAttempts)
        assertEquals(1, errors.size)
        assertTrue(errors.single().contains("有界重试"))
    }

    @Test
    fun `无法提交延迟回调时继续执行有界同步重试`() {
        val errors = mutableListOf<String>()
        var releaseAttempts = 0
        val scheduler = ForegroundLockReleaseScheduler(
            postDelayed = { _, _ -> false },
            removeCallbacks = {},
            releaseLocks = {
                releaseAttempts += 1
                true
            },
            logError = errors::add,
        )

        scheduler.release()

        assertEquals(6, releaseAttempts)
        assertTrue(errors.any { it.contains("立即执行有界重试") })
        assertTrue(errors.any { it.contains("有界重试") })
    }

    @Test
    fun `取消后已出队的旧回调不会释放新获取的物理锁`() {
        val queued = mutableListOf<Runnable>()
        var releaseAttempts = 0
        val scheduler = ForegroundLockReleaseScheduler(
            postDelayed = { runnable, _ ->
                queued += runnable
                true
            },
            removeCallbacks = { runnable -> queued.remove(runnable) },
            releaseLocks = {
                releaseAttempts += 1
                true
            },
            logError = {},
        )

        scheduler.release()
        assertEquals(1, queued.size)
        val dequeued = queued.removeAt(0)
        scheduler.cancel()

        dequeued.run()

        assertEquals(1, releaseAttempts)
    }
}
