package com.vcp.mobile.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.fail
import org.junit.Test

class SseProxyCommandTest {
    private val generationActions = listOf("prepare_resume", "resume", "cancel_resume", "stop")

    @Test
    fun `四种命令接受完整复合身份`() {
        listOf("start", "query", "prepare_resume", "resume", "cancel_resume", "stop")
            .forEach { action ->
            assertEquals(expectedKey(), SseProxyCommand.parseIdentity(action, completeFields()))
        }
    }

    @Test
    fun `四种命令拒绝 requestId 与 messageId 冲突`() {
        listOf("start", "query", "prepare_resume", "resume", "cancel_resume", "stop")
            .forEach { action ->
            assertRejects(
                action,
                completeFields().copy(messageId = "different-message")
            )
        }
    }

    @Test
    fun `四种命令拒绝不完整身份且不猜测 owner`() {
        listOf("start", "query", "prepare_resume", "resume", "cancel_resume", "stop")
            .forEach { action ->
            assertRejects(action, completeFields().copy(ownerId = null))
        }
    }

    @Test
    fun `四种命令拒绝空白身份字段`() {
        listOf("start", "query", "prepare_resume", "resume", "cancel_resume", "stop")
            .forEach { action ->
            assertRejects(action, completeFields().copy(messageId = ""))
        }
    }

    @Test
    fun `context 身份必须匹配已验证的顶层身份`() {
        assertRejects(
            "start",
            completeFields(),
            SseProxyIdentityFields(
                ownerType = "group",
                ownerId = "other-owner",
                topicId = "topic-1",
                messageId = "message-1"
            )
        )
    }

    @Test
    fun `start 和 query 允许缺少 helper generation，resume 和 stop 必须正整数`() {
        assertNull(SseProxyCommand.requireExpectedGeneration("start", null))
        assertNull(SseProxyCommand.requireExpectedGeneration("query", null))
        generationActions.forEach { action ->
            assertEquals(9L, SseProxyCommand.requireExpectedGeneration(action, 9L))
            assertRejectsGeneration(action, null)
            assertRejectsGeneration(action, 0L)
        }
    }

    @Test
    fun `start 必须携带正整数 request epoch`() {
        assertEquals(9L, SseProxyCommand.requireRequestEpoch("start", 9L))
        assertThrows(IllegalArgumentException::class.java) {
            SseProxyCommand.requireRequestEpoch("start", null)
        }
        assertThrows(IllegalArgumentException::class.java) {
            SseProxyCommand.requireRequestEpoch("start", 0L)
        }
        assertNull(SseProxyCommand.requireRequestEpoch("query", null))
    }

    private fun completeFields() = SseProxyIdentityFields(
        ownerType = "group",
        ownerId = "owner-1",
        topicId = "topic-1",
        messageId = "message-1",
        requestId = "message-1"
    )

    private fun expectedKey() = StreamSessionKey(
        ownerType = "group",
        ownerId = "owner-1",
        topicId = "topic-1",
        messageId = "message-1"
    )

    private fun assertRejects(
        action: String,
        fields: SseProxyIdentityFields,
        context: SseProxyIdentityFields? = null
    ) {
        try {
            SseProxyCommand.parseIdentity(action, fields, context)
            fail("应拒绝不完整或冲突的流身份")
        } catch (_: IllegalArgumentException) {
            // 预期拒绝，确保命令不会回退到旧的 Agent tag。
        }
    }

    private fun assertRejectsGeneration(action: String, generation: Long?) {
        try {
            SseProxyCommand.requireExpectedGeneration(action, generation)
            fail("应拒绝缺少或非正 generation")
        } catch (_: IllegalArgumentException) {
            // 预期拒绝，避免 stop/resume 退回无条件操作。
        }
    }

}
