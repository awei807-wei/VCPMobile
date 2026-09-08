package com.vcp.mobile.service

import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import android.content.Context
import android.content.ContextWrapper
import com.vcp.mobile.isWithinCanonicalDirectory
import okhttp3.Request
import okhttp3.sse.EventSource
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class StreamSessionRegistryTest {
    @Test
    fun `规范路径校验尊重目录边界`() {
        assertTrue(isWithinCanonicalDirectory("/data/cache", "/data/cache"))
        assertTrue(isWithinCanonicalDirectory("/data/cache/uploads/a.bin", "/data/cache"))
        assertFalse(isWithinCanonicalDirectory("/data/cache-old/a.bin", "/data/cache"))
        assertFalse(isWithinCanonicalDirectory("/data/cache2", "/data/cache"))
    }

    @Test
    fun `WifiLock 获取失败时回滚已获取的 WakeLock`() {
        val wake = FakeLockHandle()
        val wifi = FakeLockHandle(failAcquire = true)
        val locks = ForegroundPhysicalLocks(
            wakeLockFactory = { wake },
            wifiLockFactory = { wifi },
            logDebug = {},
            logError = { _, _ -> },
        )

        try {
            locks.acquire(FactoryContext())
            throw AssertionError("应报告 WifiLock 获取失败")
        } catch (error: IllegalStateException) {
            assertEquals(1, wake.releaseCount)
            assertEquals(0, wifi.releaseCount)
        }
    }

    @Test
    fun `WakeLock 释放异常时仍继续释放 WifiLock`() {
        val wake = FakeLockHandle(failRelease = true)
        val wifi = FakeLockHandle()
        val locks = ForegroundPhysicalLocks(
            wakeLockFactory = { wake },
            wifiLockFactory = { wifi },
            logDebug = {},
            logError = { _, _ -> },
        )

        locks.acquire(FactoryContext())
        assertFalse(locks.release())

        assertEquals(1, wake.releaseCount)
        assertEquals(1, wifi.releaseCount)
    }

    @Test
    fun `释放异常且仍持有时保留句柄并支持重试`() {
        val wake = FakeLockHandle(failRelease = true, retainOnFailure = true)
        val wifi = FakeLockHandle()
        val locks = ForegroundPhysicalLocks(
            wakeLockFactory = { wake },
            wifiLockFactory = { wifi },
            logDebug = {},
            logError = { _, _ -> },
        )

        locks.acquire(FactoryContext())
        assertTrue(locks.release())

        assertTrue(wake.isHeld)
        assertEquals(1, wake.releaseCount)
        assertEquals(1, wifi.releaseCount)

        wake.failRelease = false
        assertFalse(locks.release())

        assertFalse(wake.isHeld)
        assertEquals(2, wake.releaseCount)
    }

    @Test
    fun `SseProxy 获取锁失败后仍调度回滚释放重试`() {
        val wake = FakeLockHandle(failRelease = true, retainOnFailure = true)
        val wifi = FakeLockHandle(failAcquire = true)
        val locks = ForegroundPhysicalLocks(
            wakeLockFactory = { wake },
            wifiLockFactory = { wifi },
            logDebug = {},
            logError = { _, _ -> },
        )
        val queued = mutableListOf<Pair<Runnable, Long>>()
        val scheduler = ForegroundLockReleaseScheduler(
            postDelayed = { runnable, delay ->
                queued += runnable to delay
                true
            },
            removeCallbacks = { runnable -> queued.removeAll { it.first === runnable } },
            releaseLocks = locks::release,
            logError = {},
        )
        val keepalive = SseProxyKeepalive(FactoryContext(), locks, scheduler)

        try {
            keepalive.update(true)
            throw AssertionError("WifiLock 获取失败应向调用方报告")
        } catch (_: IllegalStateException) {
            // 获取失败后的回滚由 SseProxyKeepalive 交给有界调度器继续处理。
        }

        assertTrue(wake.isHeld)
        assertEquals(2, wake.releaseCount)
        assertEquals(listOf(100L), queued.map { it.second })

        wake.failRelease = false
        queued.removeAt(0).first.run()

        assertFalse(wake.isHeld)
        assertEquals(3, wake.releaseCount)
    }

    @Test
    fun `不同 owner 的同 messageId 保持独立`() {
        val registry = StreamSessionRegistry<Any>()
        val agentKey = key("agent", "owner-a")
        val groupKey = key("group", "owner-a")
        val agent = Any()
        val group = Any()

        val agentLease = registry.install(agentKey, agent)
        val groupLease = registry.install(groupKey, group)

        assertSame(agent, registry.current(agentKey)?.value)
        assertSame(group, registry.current(groupKey)?.value)
        assertFalse(agentLease.value === groupLease.value)
        assertTrue(registry.removeIfCurrent(agentLease))
        assertSame(group, registry.current(groupKey)?.value)
    }

    @Test
    fun `替换 generation 拒绝旧回调且只取消一次`() {
        val registry = StreamSessionRegistry<Any>()
        val key = key("agent", "owner-a")
        val cancellations = AtomicInteger(0)
        val old = registry.install(key, Any())
        val current = registry.install(key, Any()) { cancellations.incrementAndGet() }

        assertTrue(current.generation > old.generation)
        assertEquals(1, cancellations.get())
        assertFalse(registry.isCurrent(old))
        assertFalse(registry.removeIfCurrent(old))
        assertSame(current.value, registry.current(key)?.value)
    }

    @Test
    fun `重复 stop 幂等且不能移除替换后的流`() {
        val registry = StreamSessionRegistry<Any>()
        val key = key("group", "owner-g")
        val first = registry.install(key, Any())

        assertSame(first.value, registry.removeCurrent(key, first.generation)?.value)
        assertEquals(null, registry.removeCurrent(key, first.generation))

        val second = registry.install(key, Any())
        assertEquals(null, registry.removeCurrent(key, first.generation))
        assertSame(second.value, registry.current(key)?.value)
    }

    @Test
    fun `同名 Agent 使用不同前台 lease tag`() {
        val first = key("agent", "agent-a", topic = "topic-1", message = "msg-1")
        val second = key("agent", "agent-a", topic = "topic-2", message = "msg-2")

        assertTrue(StreamLeaseTag.forKey(first).startsWith("stream:session:"))
        assertFalse(StreamLeaseTag.forKey(first) == StreamLeaseTag.forKey(second))
    }

    @Test
    fun `并发安装最终保留最大 generation`() {
        val registry = StreamSessionRegistry<Any>()
        val key = key("agent", "owner-a")
        val workers = 8
        val ready = CountDownLatch(workers)
        val start = CountDownLatch(1)
        val leases = Collections.synchronizedList(mutableListOf<StreamSessionRegistry.Lease<Any>>())
        val failures = AtomicInteger(0)
        val executor = Executors.newFixedThreadPool(workers)
        repeat(workers) {
            executor.execute {
                ready.countDown()
                try {
                    if (!start.await(5, TimeUnit.SECONDS)) {
                        failures.incrementAndGet()
                    } else {
                        leases += registry.install(key, Any())
                    }
                } catch (_: InterruptedException) {
                    failures.incrementAndGet()
                }
            }
        }
        assertTrue(ready.await(5, TimeUnit.SECONDS))
        start.countDown()
        executor.shutdown()
        assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS))

        val current = registry.current(key)
        assertEquals(0, failures.get())
        assertEquals(workers, leases.size)
        assertEquals(leases.maxOf { it.generation }, current?.generation)
        assertTrue(leases.all { !registry.isCurrent(it) || it.generation == current?.generation })
    }

    @Test
    fun `旧 EventSource 在脱离跟踪前会被取消`() {
        val registry = StreamSessionRegistry<Any>()
        val session = Any()
        val lease = registry.install(key("agent", "owner-a"), session)
        val source = FakeEventSource()
        val gate = SseEventSourceInstallGate<Any, FakeEventSource>(
            registry = registry,
            sessionOf = { it.value },
            attach = { _, _ -> error("过期 source 不得安装") },
            cancel = FakeEventSource::cancel
        )

        assertTrue(registry.removeIfCurrent(lease))
        assertFalse(gate.attachIfCurrent(lease, source))
        assertEquals(1, source.cancelCount)
    }

    @Test
    fun `当前 EventSource 在 generation 门闩内安装`() {
        val registry = StreamSessionRegistry<Any>()
        val session = Any()
        val lease = registry.install(key("agent", "owner-a"), session)
        val source = FakeEventSource()
        var attached: FakeEventSource? = null
        val gate = SseEventSourceInstallGate<Any, FakeEventSource>(
            registry = registry,
            sessionOf = { it.value },
            attach = { _, value -> attached = value },
            cancel = FakeEventSource::cancel
        )

        assertTrue(gate.attachIfCurrent(lease, source))
        assertSame(source, attached)
        assertNotNull(registry.current(lease.key))
        assertEquals(0, source.cancelCount)
    }

    @Test
    fun `工厂返回前 stop 会让随后返回的 EventSource 立即取消`() {
        val registry = StreamSessionRegistry<Any>()
        val lease = registry.install(key("agent", "owner-factory"), Any())
        val entered = CountDownLatch(1)
        val releaseFactory = CountDownLatch(1)
        val executor = Executors.newSingleThreadExecutor()
        val source = FakeEventSource()
        val factory = executor.submit<FakeEventSource> {
            entered.countDown()
            assertTrue(releaseFactory.await(5, TimeUnit.SECONDS))
            source
        }
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        assertTrue(registry.removeIfCurrent(lease))
        releaseFactory.countDown()

        val gate = SseEventSourceInstallGate<Any, FakeEventSource>(
            registry = registry,
            sessionOf = { it.value },
            attach = { _, _ -> error("过期 source 不得安装") },
            cancel = FakeEventSource::cancel
        )
        assertFalse(gate.attachIfCurrent(lease, factory.get(5, TimeUnit.SECONDS)))
        assertEquals(1, source.cancelCount)
        executor.shutdownNow()
    }

    @Test
    fun `工厂返回前 replacement 会让旧 EventSource 不可安装`() {
        val registry = StreamSessionRegistry<Any>()
        val key = key("group", "owner-factory-replace")
        val oldLease = registry.install(key, Any())
        val entered = CountDownLatch(1)
        val releaseFactory = CountDownLatch(1)
        val executor = Executors.newSingleThreadExecutor()
        val source = FakeEventSource()
        val factory = executor.submit<FakeEventSource> {
            entered.countDown()
            assertTrue(releaseFactory.await(5, TimeUnit.SECONDS))
            source
        }
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        val newLease = registry.install(key, Any())
        releaseFactory.countDown()

        val gate = SseEventSourceInstallGate<Any, FakeEventSource>(
            registry = registry,
            sessionOf = { it.value },
            attach = { _, _ -> error("replacement 前的 source 不得安装") },
            cancel = FakeEventSource::cancel
        )
        assertFalse(gate.attachIfCurrent(oldLease, factory.get(5, TimeUnit.SECONDS)))
        assertEquals(1, source.cancelCount)
        assertTrue(registry.isCurrent(newLease))
        executor.shutdownNow()
    }

    @Test
    fun `前台 lease generation 与引用计数保护重叠身份`() {
        val ledger = ForegroundLeaseLedger()
        val tag = "stream:session:fixture"
        val first = ledger.acquire(tag)
        val second = ledger.acquire(tag)

        assertTrue(second > first)
        assertFalse(ledger.release(tag, first + 100))
        assertTrue(ledger.isHeld(tag))
        assertTrue(ledger.release(tag, first))
        assertTrue(ledger.isHeld(tag))
        assertTrue(ledger.release(tag, second))
        assertFalse(ledger.isHeld(tag))
    }

    @Test
    fun `旧前台 release 不能释放新 generation`() {
        val ledger = ForegroundLeaseLedger()
        val tag = "stream:session:fixture"
        val first = ledger.acquire(tag)
        assertTrue(ledger.release(tag, first))
        val current = ledger.acquire(tag)

        assertFalse(ledger.release(tag, first))
        assertTrue(ledger.isHeld(tag))
        assertTrue(ledger.release(tag, current))
        assertFalse(ledger.isHeld(tag))
    }

    @Test
    fun `前台提升失败清空所有重叠流 lease 后可重新获取`() {
        val ledger = ForegroundLeaseLedger()
        val tag = "stream:session:promotion-failure"
        val first = ledger.acquire(tag)
        val second = ledger.acquire(tag)

        ledger.clear()

        assertFalse(ledger.isHeld(tag))
        assertFalse(ledger.release(tag, first))
        val retry = ledger.acquire(tag)
        assertTrue(retry > second)
        assertTrue(ledger.release(tag, retry))
    }

    @Test
    fun `helper 引用归零后生成带 startId 的延迟停止票据`() {
        val ledger = SseProxyLifecycleLedger()
        ledger.onStartCommand(17)
        ledger.commandStarted()
        assertNull(ledger.sessionsChanged(1))
        assertNull(ledger.commandFinished())

        val token = ledger.sessionsChanged(0)
        assertEquals(SseProxyLifecycleLedger.IdleStopToken(17, token!!.epoch), token)
        assertTrue(ledger.consumeIdleStop(token))
        assertFalse(ledger.consumeIdleStop(token))
    }

    @Test
    fun `新命令会取消旧的 helper 延迟停止票据`() {
        val ledger = SseProxyLifecycleLedger()
        ledger.onStartCommand(21)
        ledger.commandStarted()
        val oldToken = ledger.commandFinished()!!

        ledger.commandStarted()
        assertFalse(ledger.consumeIdleStop(oldToken))
        val newToken = ledger.commandFinished()!!
        assertTrue(newToken.epoch > oldToken.epoch)
        assertTrue(ledger.consumeIdleStop(newToken))
    }

    @Test
    fun `helper 仅启动且没有合法命令时 startup idle 票据可自停`() {
        val ledger = SseProxyLifecycleLedger()
        val token = ledger.onStartCommand(31)

        assertNotNull(token)
        assertTrue(ledger.consumeStartupIdleStop(token!!))
        assertFalse(ledger.consumeStartupIdleStop(token))
        assertEquals(0, ledger.referenceCount())
    }

    @Test
    fun `首个合法命令转交 startup idle 票据并阻止自停`() {
        val ledger = SseProxyLifecycleLedger()
        val token = ledger.onStartCommand(32)

        assertNotNull(token)
        ledger.commandStarted()

        assertFalse(ledger.consumeStartupIdleStop(token!!))
        val idle = ledger.commandFinished()
        assertNotNull(idle)
        assertTrue(ledger.consumeIdleStop(idle!!))
    }

    @Test
    fun `命令完成后再次启动仍可消费新的 startup idle 票据`() {
        val ledger = SseProxyLifecycleLedger()
        ledger.onStartCommand(33)
        ledger.commandStarted()
        val oldIdle = ledger.commandFinished()!!

        val startup = ledger.onStartCommand(34)

        assertNotNull(startup)
        assertFalse(ledger.consumeIdleStop(oldIdle))
        assertTrue(ledger.consumeStartupIdleStop(startup!!))
    }

    @Test
    fun `有活跃 command 或 session 引用时不能创建或消费 startup idle 票据`() {
        val ledger = SseProxyLifecycleLedger()
        val initialStartup = ledger.onStartCommand(35)!!
        ledger.commandStarted()

        assertFalse(ledger.consumeStartupIdleStop(initialStartup))
        assertNull(ledger.onStartCommand(36))

        assertNull(ledger.sessionsChanged(1))
        ledger.commandFinished()
        assertNull(ledger.onStartCommand(37))

        val idle = ledger.sessionsChanged(0)
        assertNotNull(idle)
        assertTrue(ledger.consumeIdleStop(idle!!))
    }

    @Test
    fun `watchdog 键区分同 tag 的不同 generation 且旧回调可单独移除`() {
        val ledger = ForegroundWatchdogLedger<String>()
        val oldKey = ForegroundWatchdogKey("stream:session:agent", 1)
        val newKey = ForegroundWatchdogKey("stream:session:agent", 2)
        ledger.replace(oldKey, "旧超时")
        ledger.replace(newKey, "新超时")

        assertTrue(ledger.contains(oldKey))
        assertTrue(ledger.contains(newKey))
        assertEquals("旧超时", ledger.remove(oldKey))
        assertFalse(ledger.contains(oldKey))
        assertTrue(ledger.contains(newKey))
    }

    @Test
    fun `恢复文件发布门闩在 replacement 前保持旧 lease 当前`() {
        val registry = StreamSessionRegistry<Any>()
        val key = key("group", "owner-dump")
        val old = Any()
        val first = registry.install(key, old)
        val entered = CountDownLatch(1)
        val releasePublish = CountDownLatch(1)
        val replacementDone = CountDownLatch(1)
        val executor = Executors.newFixedThreadPool(2)
        val publish = executor.submit<String?> {
            registry.withCurrent(first) {
                entered.countDown()
                assertTrue(releasePublish.await(5, TimeUnit.SECONDS))
                "A"
            }
        }
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        val replacement = executor.submit<StreamSessionRegistry.Lease<Any>> {
            val lease = registry.install(key, Any())
            replacementDone.countDown()
            lease
        }

        releasePublish.countDown()
        assertEquals("A", publish.get(5, TimeUnit.SECONDS))
        val replacementLease = replacement.get(5, TimeUnit.SECONDS)
        assertTrue(replacementDone.await(5, TimeUnit.SECONDS))
        executor.shutdownNow()
        assertFalse(registry.isCurrent(first))
        assertTrue(registry.isCurrent(replacementLease))
    }

    @Test
    fun `恢复文件首次失败期间保持 helper 引用`() {
        val ledger = SseDumpRetryLedger(maxAttempts = 3)

        assertEquals(SseDumpRetryLedger.BeginResult.STARTED, ledger.beginAttempt())
        assertEquals(SseDumpRetryLedger.FinishResult.RETRY_SCHEDULED, ledger.finishAttempt(false))
        assertTrue(ledger.hasPendingReference())
        assertTrue(ledger.markRetryReady())
    }

    @Test
    fun `恢复文件达到最大重试后可观测耗尽并释放引用`() {
        val ledger = SseDumpRetryLedger(maxAttempts = 2)

        assertEquals(SseDumpRetryLedger.BeginResult.STARTED, ledger.beginAttempt())
        assertEquals(SseDumpRetryLedger.FinishResult.RETRY_SCHEDULED, ledger.finishAttempt(false))
        assertTrue(ledger.markRetryReady())
        assertEquals(SseDumpRetryLedger.BeginResult.STARTED, ledger.beginAttempt())
        assertEquals(SseDumpRetryLedger.FinishResult.EXHAUSTED, ledger.finishAttempt(false))
        assertFalse(ledger.hasPendingReference())
        assertEquals(SseDumpRetryLedger.BeginResult.EXHAUSTED, ledger.beginAttempt())
    }

    @Test
    fun `恢复 token 与 Rust fixture 完全一致`() {
        val key = StreamSessionKey("group", "owner/1", "topic:1", "message-1")

        assertEquals("5:group7:owner/17:topic:19:message-1", key.canonicalValue())
        assertEquals(
            "082b896ff27af4a02a8feb019daf3dd40fded7e590357528de4116da79d8dec6",
            key.stableToken()
        )
        assertEquals(
            "stream:session:67726f7570:6f776e65722f31:746f7069633a31:6d6573736167652d31",
            StreamLeaseTag.forKey(key)
        )
    }

    private class FakeEventSource : EventSource {
        var cancelCount = 0

        override fun request(): Request = Request.Builder()
            .url("http://localhost")
            .build()

        override fun cancel() {
            cancelCount += 1
        }
    }

    private class FakeLockHandle(
        private val failAcquire: Boolean = false,
        var failRelease: Boolean = false,
        private val retainOnFailure: Boolean = false,
    ) : ForegroundLockHandle {
        override var isHeld: Boolean = false
        var releaseCount = 0

        override fun acquire() {
            if (failAcquire) throw IllegalStateException("注入 WifiLock 获取失败")
            isHeld = true
        }

        override fun release() {
            releaseCount += 1
            if (failRelease) {
                if (!retainOnFailure) isHeld = false
                throw IllegalStateException("注入 WakeLock 释放失败")
            }
            isHeld = false
        }
    }

    private class FactoryContext : ContextWrapper(null) {
        override fun getApplicationContext(): Context = this
    }

    private fun key(
        ownerType: String,
        ownerId: String,
        topic: String = "topic-1",
        message: String = "same-message"
    ) = StreamSessionKey(ownerType, ownerId, topic, message)
}
