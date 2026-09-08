package com.vcp.mobile.service

import android.content.Context
import android.content.ContextWrapper
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.OutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancel
import okhttp3.Request
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.assertThrows
import org.junit.Test

class SseSessionManagerTest {
    @Test
    fun `公开 start 工厂阻塞期间 replacement 取消旧 generation`() {
        val key = StreamSessionKey("agent", "owner-a", "topic-a", "message-a")
        val firstEntered = CountDownLatch(1)
        val releaseFirst = CountDownLatch(1)
        val calls = AtomicInteger(0)
        val firstSource = RecordingEventSource()
        val secondSource = RecordingEventSource()
        val factory = SseEventSourceFactory { _, _ ->
            if (calls.getAndIncrement() == 0) {
                firstEntered.countDown()
                assertTrue(releaseFirst.await(5, TimeUnit.SECONDS))
                firstSource
            } else {
                secondSource
            }
        }
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {},
            appInForeground = { true },
            eventSourceFactory = factory,
        )
        val executor = Executors.newSingleThreadExecutor()
        val firstOutput = ByteArrayOutputStream()
        val firstStart = executor.submit<SseSocketLease> {
            manager.start(startCommand(key), firstOutput)
        }
        if (!firstEntered.await(5, TimeUnit.SECONDS)) {
            try {
                firstStart.get(1, TimeUnit.SECONDS)
            } catch (error: Exception) {
                throw AssertionError("首次 start 未进入工厂: $error", error)
            }
            throw AssertionError("首次 start 未进入工厂且未失败")
        }

        val secondOutput = ByteArrayOutputStream()
        val secondLease = manager.start(startCommand(key, requestEpoch = 2L), secondOutput)
        assertEquals(2L, secondLease.sessionLease.generation)
        assertEquals(2L, frameLong(secondOutput, "generation"))

        releaseFirst.countDown()
        try {
            firstStart.get(5, TimeUnit.SECONDS)
            throw AssertionError("replacement 后旧 start 不得成功")
        } catch (_: ExecutionException) {
            // 旧 factory 返回后，安装门闩必须拒绝并精确取消该 source。
        }
        assertEquals(1, firstSource.cancelCount.get())
        assertEquals(0, secondSource.cancelCount.get())

        manager.stop(key, 1L)
        assertEquals(0, secondSource.cancelCount.get())
        manager.stop(key, secondLease.sessionLease.generation)
        manager.stop(key, secondLease.sessionLease.generation)
        assertEquals(1, secondSource.cancelCount.get())
        assertFalse(manager.hasRunningSessions())

        executor.shutdownNow()
        scope.cancel()
    }

    @Test
    fun `公开 query resume stop 严格校验 generation 并返回 ACK`() {
        val key = StreamSessionKey("group", "owner-g", "topic-g", "message-g")
        val source = RecordingEventSource()
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {},
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ -> source },
        )
        val startOutput = TrackingOutputStream()
        val lease = manager.start(startCommand(key), startOutput)
        assertEquals(1L, frameLong(startOutput, "generation"))

        val queryOutput = ByteArrayOutputStream()
        manager.query(key, queryOutput)
        val query = frame(queryOutput)
        assertEquals("streaming", frameField(query, "status"))
        assertEquals(lease.sessionLease.generation, frameLong(query, "generation"))

        val missingQueryOutput = ByteArrayOutputStream()
        val missingKey = StreamSessionKey("agent", "owner-missing", "topic-missing", "message-missing")
        manager.query(missingKey, missingQueryOutput)
        val missingQuery = frame(missingQueryOutput)
        assertEquals("not_found", frameField(missingQuery, "status"))
        assertTrue(missingQuery.contains("\"generation\":null"))
        assertTrue(missingQuery.contains("\"requestId\":\"message-missing\""))
        assertTrue(missingQuery.contains("\"ownerType\":\"agent\""))
        assertTrue(missingQuery.contains("\"ownerId\":\"owner-missing\""))
        assertTrue(missingQuery.contains("\"topicId\":\"topic-missing\""))

        val staleResumeOutput = ByteArrayOutputStream()
        assertNull(
            manager.resume(
                key,
                0,
                staleResumeOutput,
                lease.sessionLease.generation + 1,
            ),
        )
        assertEquals("error", frameField(frame(staleResumeOutput), "eventType"))
        assertTrue(frame(staleResumeOutput).contains("\"generation\":null"))
        assertEquals(0, source.cancelCount.get())

        val resumeOutput = TrackingOutputStream()
        val resumed = manager.resume(
            key,
            0,
            resumeOutput,
            lease.sessionLease.generation,
        )
        assertNotNull(resumed)
        assertEquals(lease.sessionLease.generation, frameLong(resumeOutput, "generation"))
        assertTrue(startOutput.closed)
        assertEquals(1, startOutput.closeCount)

        manager.stop(key, lease.sessionLease.generation + 1)
        assertTrue(manager.hasRunningSessions())
        assertEquals(0, source.cancelCount.get())
        manager.stop(key, lease.sessionLease.generation)
        assertEquals(1, source.cancelCount.get())
        assertTrue(resumeOutput.closed)
        assertEquals(1, resumeOutput.closeCount)
        assertFalse(manager.hasRunningSessions())

        scope.cancel()
    }

    @Test
    fun `找不到 helper session 的真实错误帧显式携带 null generation`() {
        val key = StreamSessionKey("agent", "owner-missing", "topic-missing", "message-missing")
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {},
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ -> RecordingEventSource() },
        )
        val output = ByteArrayOutputStream()

        assertNull(manager.resume(key, 0, output, expectedGeneration = 12L))

        val response = frame(output)
        assertTrue(response.contains("\"eventType\":\"error\""))
        assertTrue(response.contains("\"generation\":null"))
        assertTrue(response.contains("\"requestId\":\"message-missing\""))
        assertTrue(response.contains("\"ownerType\":\"agent\""))
        assertTrue(response.contains("\"ownerId\":\"owner-missing\""))
        assertTrue(response.contains("\"topicId\":\"topic-missing\""))

        scope.cancel()
    }

    @Test
    fun `异常 socket EOF 保留有界恢复租约并允许 query resume`() {
        val key = StreamSessionKey("agent", "owner-a", "topic-a", "message-a")
        val source = RecordingEventSource()
        val sessionsChanged = AtomicInteger(0)
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = { sessionsChanged.incrementAndGet() },
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ -> source },
        )
        val output = TrackingOutputStream()
        val lease = manager.start(startCommand(key), output)
        val startedChanges = sessionsChanged.get()

        manager.onSocketDisconnected(lease)

        assertTrue(output.closed)
        assertEquals(0, source.cancelCount.get())
        assertTrue(manager.hasRunningSessions())
        assertEquals(1, manager.runningSessionCount())
        assertTrue(sessionsChanged.get() > startedChanges)

        val queryOutput = ByteArrayOutputStream()
        manager.query(key, queryOutput)
        val query = frame(queryOutput)
        assertEquals("streaming", frameField(query, "status"))
        assertEquals(lease.sessionLease.generation, frameLong(query, "generation"))

        val resumeOutput = TrackingOutputStream()
        val resumed = manager.resume(
            key,
            0,
            resumeOutput,
            lease.sessionLease.generation,
        )
        assertNotNull(resumed)
        assertEquals(0, source.cancelCount.get())
        assertTrue(manager.hasRunningSessions())

        // 旧 EOF 或 Rust 随后的精确 stop 只能作用于当前复合身份/generation。
        manager.onSocketDisconnected(lease)
        manager.stop(key, lease.sessionLease.generation)
        assertEquals(1, source.cancelCount.get())
        assertTrue(resumeOutput.closed)
        assertFalse(manager.hasRunningSessions())

        scope.cancel()
    }

    @Test
    fun `异常 EOF 后候选 resume 失败仍保留恢复租约`() {
        val key = StreamSessionKey("agent", "owner-retry", "topic-retry", "message-retry")
        val source = RecordingEventSource()
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {},
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ -> source },
        )
        val oldOutput = TrackingOutputStream()
        val lease = manager.start(startCommand(key), oldOutput)
        manager.onSocketDisconnected(lease)

        assertNull(
            manager.resume(
                key,
                0,
                FailingOutputStream(failOnWrite = 1),
                lease.sessionLease.generation,
            )
        )
        assertTrue(manager.hasRunningSessions())
        assertNotNull(lease.sessionLease.value.pendingTakeoverToken)
        assertTrue(lease.sessionLease.value.pendingTakeoverConnectionLost)
        assertEquals(0, source.cancelCount.get())

        manager.stop(key, lease.sessionLease.generation)
        assertEquals(1, source.cancelCount.get())
        assertFalse(manager.hasRunningSessions())
        scope.cancel()
    }

    @Test
    fun `准备同 generation 接管时旧 socket EOF 只释放连接不删除 session`() {
        val key = StreamSessionKey("agent", "owner-a", "topic-a", "message-a")
        val source = RecordingEventSource()
        val sessionsChanged = AtomicInteger(0)
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = { sessionsChanged.incrementAndGet() },
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ -> source },
        )
        val oldOutput = TrackingOutputStream()
        val lease = manager.start(startCommand(key), oldOutput)
        val changesBeforeCancel = sessionsChanged.get()

        assertTrue(manager.prepareResume(key, lease.sessionLease.generation))
        manager.onSocketDisconnected(lease)

        assertFalse(oldOutput.closed)
        assertEquals(0, source.cancelCount.get())
        assertTrue(manager.hasRunningSessions())
        assertEquals(1, manager.runningSessionCount())

        assertTrue(manager.cancelResume(key, lease.sessionLease.generation))
        assertTrue(oldOutput.closed)
        assertEquals(1, oldOutput.closeCount)
        assertEquals(1, source.cancelCount.get())
        assertFalse(manager.hasRunningSessions())
        assertTrue(sessionsChanged.get() > changesBeforeCancel)

        scope.cancel()
    }

    @Test
    fun `旧 socket 健康时取消 takeover 只解除 pending 并保留 session`() {
        val key = StreamSessionKey("group", "owner-healthy", "topic-healthy", "message-healthy")
        val wrongKey = StreamSessionKey("group", "other-owner", "topic-healthy", "message-healthy")
        val source = RecordingEventSource()
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {},
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ -> source },
        )
        val oldOutput = TrackingOutputStream()
        val lease = manager.start(startCommand(key), oldOutput)
        val session = lease.sessionLease.value

        assertTrue(manager.prepareResume(key, lease.sessionLease.generation))
        assertFalse(manager.cancelResume(key, lease.sessionLease.generation + 1))
        assertNotNull(session.pendingTakeoverToken)
        assertFalse(manager.cancelResume(wrongKey, lease.sessionLease.generation))
        assertNotNull(session.pendingTakeoverToken)

        assertTrue(manager.cancelResume(key, lease.sessionLease.generation))
        assertNull(session.pendingTakeoverToken)
        assertNull(session.pendingTakeoverConnectionGeneration)
        assertFalse(session.pendingTakeoverConnectionLost)
        assertTrue(manager.hasRunningSessions())
        assertFalse(oldOutput.closed)
        assertEquals(0, source.cancelCount.get())

        manager.onSocketDisconnected(lease)
        assertTrue(manager.hasRunningSessions())
        assertEquals(0, source.cancelCount.get())
        manager.stop(key, lease.sessionLease.generation)
        assertFalse(manager.hasRunningSessions())
        assertEquals(1, source.cancelCount.get())
        scope.cancel()
    }

    @Test
    fun `旧 socket transient EOF 后同 generation resume 成功接管`() {
        val key = StreamSessionKey("group", "owner-g", "topic-g", "message-g")
        val source = RecordingEventSource()
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {},
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ -> source },
        )
        val oldOutput = TrackingOutputStream()
        val lease = manager.start(startCommand(key), oldOutput)
        assertTrue(manager.prepareResume(key, lease.sessionLease.generation))

        // This is the EOF/read-error callback from Rust's old reader.  It must
        // not race the candidate resume out of the registry.
        manager.onSocketDisconnected(lease)

        val newOutput = TrackingOutputStream()
        val resumed = manager.resume(
            key,
            0,
            newOutput,
            lease.sessionLease.generation,
        )
        assertNotNull(resumed)
        assertTrue(oldOutput.closed)
        assertEquals(1, oldOutput.closeCount)
        assertFalse(newOutput.closed)
        assertEquals(0, source.cancelCount.get())
        assertNull(lease.sessionLease.value.pendingTakeoverToken)
        assertNull(lease.sessionLease.value.pendingTakeoverConnectionGeneration)
        assertFalse(lease.sessionLease.value.pendingTakeoverConnectionLost)

        // The old connection lease is stale after commit and cannot remove the
        // newly attached socket/session.
        manager.onSocketDisconnected(lease)
        assertTrue(manager.hasRunningSessions())

        manager.stop(key, lease.sessionLease.generation)
        assertTrue(newOutput.closed)
        assertEquals(1, newOutput.closeCount)
        assertEquals(1, source.cancelCount.get())
        scope.cancel()
    }

    @Test
    fun `低 epoch 的迟到 start 不得覆盖先到的新请求`() {
        val key = StreamSessionKey("agent", "owner-epoch", "topic-epoch", "message-epoch")
        val newSource = RecordingEventSource()
        val oldSource = RecordingEventSource()
        val scope = CoroutineScope(Dispatchers.Default)
        var sourceCalls = 0
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {},
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ ->
                if (sourceCalls++ == 0) newSource else oldSource
            },
        )

        val current = manager.start(startCommand(key, requestEpoch = 2L), TrackingOutputStream())
        assertThrows(IllegalStateException::class.java) {
            manager.start(startCommand(key, requestEpoch = 1L), TrackingOutputStream())
        }
        assertEquals(0, newSource.cancelCount.get())
        assertEquals(2L, current.sessionLease.value.requestEpoch)

        manager.stop(key, current.sessionLease.generation)
        assertEquals(1, newSource.cancelCount.get())
        assertEquals(0, oldSource.cancelCount.get())
        scope.cancel()
    }

    @Test
    fun `高 epoch 的后到 start 替换旧流且旧 stop 不得误杀新流`() {
        val key = StreamSessionKey("group", "owner-epoch-2", "topic-epoch-2", "message-epoch-2")
        val oldSource = RecordingEventSource()
        val newSource = RecordingEventSource()
        val scope = CoroutineScope(Dispatchers.Default)
        var sourceCalls = 0
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {},
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ ->
                if (sourceCalls++ == 0) oldSource else newSource
            },
        )

        val old = manager.start(startCommand(key, requestEpoch = 1L), TrackingOutputStream())
        val current = manager.start(startCommand(key, requestEpoch = 2L), TrackingOutputStream())
        assertEquals(1, oldSource.cancelCount.get())
        assertEquals(0, newSource.cancelCount.get())

        manager.stop(key, old.sessionLease.generation)
        assertEquals(0, newSource.cancelCount.get())
        manager.stop(key, current.sessionLease.generation)
        assertEquals(1, newSource.cancelCount.get())
        scope.cancel()
    }

    @Test
    fun `resume 更新锁失败时按 socket 断开清理 registry 和 helper 流`() {
        val key = StreamSessionKey("agent", "owner-a", "topic-a", "message-a")
        val source = RecordingEventSource()
        var failOnSessionsChanged = false
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = {
                if (failOnSessionsChanged) {
                    throw IllegalStateException("注入物理保活锁更新失败")
                }
            },
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, _ -> source },
        )
        val startOutput = TrackingOutputStream()
        val lease = manager.start(startCommand(key), startOutput)
        failOnSessionsChanged = true
        val resumeOutput = TrackingOutputStream()

        try {
            manager.resume(key, 0, resumeOutput, lease.sessionLease.generation)
            throw AssertionError("锁更新失败时 resume 必须报告错误")
        } catch (_: IllegalStateException) {
            // resume 的新 socket 已按 onSocketDisconnected 语义关闭，旧 helper
            // EventSource 与 registry 也必须一起清理，避免 FGS 永久保持。
        }

        assertTrue(resumeOutput.closed)
        assertEquals(1, startOutput.closeCount)
        assertEquals(1, resumeOutput.closeCount)
        assertEquals(1, source.cancelCount.get())
        assertFalse(manager.hasRunningSessions())
        assertEquals(0, manager.runningSessionCount())

        failOnSessionsChanged = false
        manager.stop(key, lease.sessionLease.generation)
        assertEquals(1, source.cancelCount.get())
        scope.cancel()
    }

    @Test
    fun `resume ACK 写失败时只回滚候选连接并保留旧 session`() {
        val key = StreamSessionKey("agent", "owner-a", "topic-a", "message-a")
        val source = RecordingEventSource()
        val sessionsChanged = AtomicInteger(0)
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = { sessionsChanged.incrementAndGet() },
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, listener ->
                source.listener = listener
                source
            },
        )
        val startLeaseOutput = TrackingOutputStream()
        val startLease = manager.start(startCommand(key), startLeaseOutput)
        val changesBeforeFailure = sessionsChanged.get()
        val failedOutput = FailingOutputStream(failOnWrite = 1)

        assertNull(manager.resume(key, 0, failedOutput, startLease.sessionLease.generation))

        assertTrue(failedOutput.closed)
        assertFalse(startLeaseOutput.closed)
        assertEquals(0, startLeaseOutput.closeCount)
        assertEquals(1, failedOutput.closeCount)
        assertEquals(0, source.cancelCount.get())
        assertTrue(manager.hasRunningSessions())
        assertEquals(1, manager.runningSessionCount())
        assertEquals(changesBeforeFailure, sessionsChanged.get())

        // 只有明确 stop 才能销毁恢复失败后仍健康的旧 session。
        manager.stop(key, startLease.sessionLease.generation)
        assertEquals(1, source.cancelCount.get())
        assertTrue(startLeaseOutput.closed)
        assertEquals(1, startLeaseOutput.closeCount)
        assertFalse(manager.hasRunningSessions())

        scope.cancel()
    }

    @Test
    fun `resume 回放写失败时只回滚候选连接并允许旧 session继续`() {
        val key = StreamSessionKey("group", "owner-g", "topic-g", "message-g")
        val firstSource = RecordingEventSource()
        val replacementSource = RecordingEventSource()
        val sourceCalls = AtomicInteger(0)
        val sessionsChanged = AtomicInteger(0)
        val scope = CoroutineScope(Dispatchers.Default)
        val manager = SseSessionManager(
            FactoryContext(),
            scope,
            onSessionsChanged = { sessionsChanged.incrementAndGet() },
            appInForeground = { true },
            eventSourceFactory = SseEventSourceFactory { _, listener ->
                if (sourceCalls.getAndIncrement() == 0) {
                    firstSource.listener = listener
                    firstSource
                } else {
                    replacementSource.listener = listener
                    replacementSource
                }
            },
        )
        val firstLeaseOutput = TrackingOutputStream()
        val firstLease = manager.start(startCommand(key), firstLeaseOutput)
        firstLease.sessionLease.value.eventBuffer.add(
            object : org.json.JSONObject() {
                override fun toString(): String =
                    "{\"eventType\":\"message\",\"eventData\":\"payload-1\"}"
            }
        )
        firstLease.sessionLease.value.eventBuffer.add(
            object : org.json.JSONObject() {
                override fun toString(): String =
                    "{\"eventType\":\"message\",\"eventData\":\"payload-2\"}"
            }
        )
        val changesBeforeFailure = sessionsChanged.get()

        // ACK 占用第一帧；第一条回放成功，第二条回放失败，覆盖中途
        // replay 失败而旧 socket 仍需继续存活的路径。
        val failedReplayOutput = FailingOutputStream(failOnWrite = 3)
        assertNull(
            manager.resume(
                key,
                0,
                failedReplayOutput,
                firstLease.sessionLease.generation,
            )
        )

        assertTrue(failedReplayOutput.closed)
        assertFalse(firstLeaseOutput.closed)
        assertEquals(0, firstLeaseOutput.closeCount)
        assertEquals(1, failedReplayOutput.closeCount)
        assertEquals(0, firstSource.cancelCount.get())
        assertTrue(manager.hasRunningSessions())
        assertEquals(1, manager.runningSessionCount())
        assertEquals(changesBeforeFailure, sessionsChanged.get())

        // 同一 generation 可以在候选失败后再次接管旧 session。
        val retryOutput = TrackingOutputStream()
        val retryLease = manager.resume(
            key,
            0,
            retryOutput,
            firstLease.sessionLease.generation,
        )
        assertNotNull(retryLease)
        assertTrue(firstLeaseOutput.closed)
        assertEquals(1, firstLeaseOutput.closeCount)
        manager.stop(key, firstLease.sessionLease.generation)
        assertTrue(retryOutput.closed)
        assertEquals(1, retryOutput.closeCount)
        assertEquals(1, firstSource.cancelCount.get())

        // 旧 binding 晚到时只能命中 stale 分支，不重复取消 helper。
        assertNull(
            manager.resume(
                key,
                0,
                ByteArrayOutputStream(),
                firstLease.sessionLease.generation,
            )
        )
        assertEquals(1, firstSource.cancelCount.get())

        val replacementLease = manager.start(
            startCommand(key, requestEpoch = 2L),
            TrackingOutputStream(),
        )
        assertEquals(2L, replacementLease.sessionLease.generation)
        assertEquals(0, replacementSource.cancelCount.get())
        assertEquals(1, manager.runningSessionCount())
        val changesAfterReplacement = sessionsChanged.get()

        // 失败 resume 的旧 binding 晚到时只能命中 stale 分支，不能碰新流。
        manager.onSocketDisconnected(firstLease)
        assertEquals(0, replacementSource.cancelCount.get())
        assertEquals(changesAfterReplacement, sessionsChanged.get())

        manager.stop(key, replacementLease.sessionLease.generation)
        manager.onSocketDisconnected(firstLease)
        assertEquals(1, replacementSource.cancelCount.get())
        assertEquals(0, manager.runningSessionCount())

        scope.cancel()
    }

    private fun startCommand(
        key: StreamSessionKey,
        requestEpoch: Long = 1L,
    ): SseProxyCommand {
        return SseProxyCommand(
            action = "start",
            key = key,
            url = "http://localhost/v1/chat/completions",
            requestEpoch = requestEpoch,
        )
    }

    private fun frame(output: ByteArrayOutputStream): String {
        val value = SseSocketCodec.read(ByteArrayInputStream(output.toByteArray()))
        assertNotNull(value)
        return value!!
    }

    private fun frameField(json: String, name: String): String {
        val match = Regex("\\\"$name\\\":\\\"([^\\\"]*)\\\"").find(json)
        assertNotNull(match)
        return match!!.groupValues[1]
    }

    private fun frameLong(output: ByteArrayOutputStream, name: String): Long {
        return frameLong(frame(output), name)
    }

    private fun frameLong(json: String, name: String): Long {
        val match = Regex("\\\"$name\\\":([0-9]+)").find(json)
        assertNotNull(match)
        return match!!.groupValues[1].toLong()
    }

    private class RecordingEventSource : EventSource {
        val cancelCount = AtomicInteger(0)
        var listener: EventSourceListener? = null

        fun emit(data: String) {
            listener?.onEvent(this, null, null, data)
        }

        override fun request(): Request = Request.Builder()
            .url("http://localhost")
            .build()

        override fun cancel() {
            cancelCount.incrementAndGet()
        }
    }

    private class TrackingOutputStream : ByteArrayOutputStream() {
        var closed = false
        var closeCount = 0

        override fun close() {
            closed = true
            closeCount += 1
            super.close()
        }
    }

    private class FailingOutputStream(
        private val failOnWrite: Int,
    ) : OutputStream() {
        private var writeCount = 0
        var closed = false
            private set
        var closeCount = 0
            private set

        override fun write(value: Int) {
            write(byteArrayOf(value.toByte()))
        }

        override fun write(bytes: ByteArray, offset: Int, length: Int) {
            writeCount += 1
            if (writeCount >= failOnWrite) {
                throw IOException("注入输出失败")
            }
        }

        override fun close() {
            closed = true
            closeCount += 1
        }
    }

    private class FactoryContext : ContextWrapper(null) {
        override fun getApplicationContext(): Context = this
    }
}
