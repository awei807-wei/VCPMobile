package com.vcp.mobile.service

/** 纯 JVM 可测的 helper 引用与延迟自停账本。 */
internal class SseProxyLifecycleLedger {
    data class IdleStopToken(val startId: Int, val epoch: Long)
    data class StartupIdleStopToken(val startId: Int, val epoch: Long)

    private var inFlightCommands = 0
    private var runningSessions = 0
    private var latestStartId = 0
    private var epoch = 0L
    private var idleStopPending = false
    private var startupIdleStopPending = false
    private var wasReferenced = false

    @Synchronized
    fun onStartCommand(startId: Int): StartupIdleStopToken? {
        latestStartId = startId
        epoch++
        idleStopPending = false
        if (inFlightCommands + runningSessions != 0) {
            startupIdleStopPending = false
            return null
        }
        startupIdleStopPending = true
        return StartupIdleStopToken(startId, epoch)
    }

    @Synchronized
    fun commandStarted() {
        inFlightCommands++
        startupIdleStopPending = false
        wasReferenced = true
        epoch++
        idleStopPending = false
    }

    @Synchronized
    fun commandFinished(): IdleStopToken? {
        if (inFlightCommands > 0) {
            inFlightCommands--
        }
        return idleStopTokenLocked()
    }

    @Synchronized
    fun sessionsChanged(count: Int): IdleStopToken? {
        runningSessions = count.coerceAtLeast(0)
        if (runningSessions > 0) {
            epoch++
            idleStopPending = false
            return null
        }
        return idleStopTokenLocked()
    }

    @Synchronized
    fun consumeIdleStop(token: IdleStopToken): Boolean {
        if (!idleStopPending || token.epoch != epoch || token.startId != latestStartId) {
            return false
        }
        if (inFlightCommands != 0 || runningSessions != 0) {
            idleStopPending = false
            return false
        }
        idleStopPending = false
        return true
    }

    /**
     * 消费仅在 helper 启动后等待首个合法命令时创建的自停票据。
     *
     * 该票据与普通引用归零票据分开，避免一个无效/半连接 socket
     * 把 helper 永久留在前台；首个合法命令由 [commandStarted] 原子地
     * 取消 startup 票据并转入正常 command/session 引用计数。
     */
    @Synchronized
    fun consumeStartupIdleStop(token: StartupIdleStopToken): Boolean {
        if (
            !startupIdleStopPending ||
            token.epoch != epoch ||
            token.startId != latestStartId ||
            inFlightCommands != 0 ||
            runningSessions != 0
        ) {
            return false
        }
        startupIdleStopPending = false
        return true
    }

    @Synchronized
    fun referenceCount(): Int = inFlightCommands + runningSessions

    private fun idleStopTokenLocked(): IdleStopToken? {
        if (!wasReferenced || idleStopPending || inFlightCommands + runningSessions != 0) {
            return null
        }
        epoch++
        idleStopPending = true
        return IdleStopToken(latestStartId, epoch)
    }
}
