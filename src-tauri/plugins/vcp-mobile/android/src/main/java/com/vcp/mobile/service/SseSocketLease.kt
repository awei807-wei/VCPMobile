package com.vcp.mobile.service

/** 将一个 socket 连接绑定到流 generation 与连接 lease。 */
data class SseSocketLease(
    val sessionLease: StreamSessionRegistry.Lease<SseStreamSession>,
    val connectionGeneration: Long
)
