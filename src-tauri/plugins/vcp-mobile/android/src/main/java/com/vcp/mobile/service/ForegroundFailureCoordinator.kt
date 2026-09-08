package com.vcp.mobile.service

import android.content.Context

/** 前台刷新失败后的统一收口顺序：先撤销所有权，再清除持久意图，最后协调服务自停。 */
internal class ForegroundFailureCoordinator(
    private val clearRuntimeState: () -> Unit,
    private val clearPersistence: (Context) -> Unit,
    private val reconcileService: (Context) -> Unit,
) {
    fun clear(context: Context) {
        clearRuntimeState()
        clearPersistence(context)
        reconcileService(context)
    }
}
