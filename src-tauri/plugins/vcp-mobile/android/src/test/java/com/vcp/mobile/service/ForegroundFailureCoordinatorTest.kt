package com.vcp.mobile.service

import android.content.Context
import android.content.ContextWrapper
import org.junit.Assert.assertEquals
import org.junit.Test

class ForegroundFailureCoordinatorTest {
    @Test
    fun `刷新失败按撤销持久意图再协调服务的顺序收口`() {
        val calls = mutableListOf<String>()
        val coordinator = ForegroundFailureCoordinator(
            clearRuntimeState = { calls += "runtime" },
            clearPersistence = { calls += "persistence" },
            reconcileService = { calls += "reconcile" },
        )

        coordinator.clear(TestContext())

        assertEquals(listOf("runtime", "persistence", "reconcile"), calls)
    }

    private class TestContext : ContextWrapper(null) {
        override fun getApplicationContext(): Context = this
    }
}
