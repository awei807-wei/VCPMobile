package com.vcp.mobile.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ForegroundTagContractTest {
    @Test
    fun `分布式样例 tag 在 Android 侧保持同一命名空间`() {
        val samples = listOf(
            "distributed",
            "distributed:generation:7",
            "distributed:connect:session:7:generation:7",
            "distributed:connection:session:7:generation:7",
            "distributed:tool:req-7:session:7:generation:7",
            "distributed:placeholder_push:session:7:generation:7",
        )

        samples.forEach { tag ->
            assertTrue(tag, ForegroundTagContract.isDistributedTag(tag))
        }
        assertFalse(ForegroundTagContract.isDistributedTag("distributedx"))
        assertTrue(
            ForegroundTagContract.isGenerationDistributedTag(
                "distributed:connect:session:7:generation:7",
            ),
        )
        assertFalse(ForegroundTagContract.isGenerationDistributedTag("distributed"))
    }
}
