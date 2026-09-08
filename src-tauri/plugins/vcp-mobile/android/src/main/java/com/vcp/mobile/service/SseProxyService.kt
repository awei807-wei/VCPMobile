package com.vcp.mobile.service

import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.IBinder
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.joinAll
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.Collections

/**
 * 独立 helper 进程中的本地 TCP SSE 代理。
 *
 * 服务只负责前台生命周期、socket 接入和物理保活；会话所有权由
 * [SseSessionManager] 负责，避免 requestId 跨 owner 串流。
 */
class SseProxyService : Service() {
    companion object {
        private const val TAG = "VcpSseProxy"
        const val CHANNEL_ID = "vcp_sse_proxy_helper"
        const val NOTIFICATION_ID_SERVICE = 0x53545202
        private const val IDLE_STOP_DELAY_MS = 250L
        // Rust 等待 helper 端口最多三秒；startup lease 略长于该窗口，
        // 确保正常冷启动可以在 helper 自停前送达首个命令。
        private const val STARTUP_IDLE_TIMEOUT_MS = 5_000L
        private const val INITIAL_COMMAND_READ_TIMEOUT_MS = 4_000
        // 合法 SSE 命令会持有 socket 直至流结束；零值是协议约定的无限
        // 空闲读取超时，Rust 侧仍会独立限制命令握手和恢复查询。
        private const val STREAM_SOCKET_READ_TIMEOUT_MS = 0

        @Volatile
        var isServiceRunning = false
    }

    private val serviceScope = CoroutineScope(Dispatchers.Default + SupervisorJob())
    private val helperToken = SseProxyAuth.generateToken()
    private val lifecycleLedger = SseProxyLifecycleLedger()
    private val foreground = SseProxyServiceForeground(this)
    private var serverSocket: ServerSocket? = null
    private var serverJob: Job? = null
    private val endpointPublishJobs = Collections.synchronizedSet(mutableSetOf<Job>())
    @Volatile
    private var destroying = false
    @Volatile
    private var serverPort: Int? = null
    private var foregroundPromoted = false
    private var isTaskRemoved = false
    private lateinit var sessionManager: SseSessionManager
    private lateinit var keepalive: SseProxyKeepalive

    override fun onCreate() {
        super.onCreate()
        foreground.createNotificationChannel()
        foregroundPromoted = foreground.promote(foreground.buildServiceNotification())
        if (!foregroundPromoted) {
            Log.e(TAG, "前台提升失败，停止 helper")
            stopSelf()
            return
        }

        isServiceRunning = true
        try {
            keepalive = SseProxyKeepalive(this)
            sessionManager = SseSessionManager(
                context = this,
                scope = serviceScope,
                onSessionsChanged = ::updateLocks,
                appInForeground = ::isAppInForeground
            )
            startTcpServer()
        } catch (error: Exception) {
            failService("初始化 helper 失败", error)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (!foregroundPromoted) {
            stopSelf(startId)
            return START_NOT_STICKY
        }
        lifecycleLedger.onStartCommand(startId)?.let(::scheduleStartupIdleStop)
        serverSocket?.let { server ->
            publishEndpoint(server.localPort)
        }
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        Log.i(TAG, "onDestroy：关闭 TCP 服务和全部会话")
        isServiceRunning = false
        removeForeground()
        val jobs = synchronized(endpointPublishJobs) {
            destroying = true
            val allJobs = buildList {
                serverJob?.let(::add)
                addAll(endpointPublishJobs)
            }
            allJobs.forEach(Job::cancel)
            allJobs
        }
        try {
            serverSocket?.close()
        } catch (_error: Exception) {
            Log.w(TAG, "关闭 helper TCP 服务失败：result=error")
        }
        runBlocking { jobs.joinAll() }
        serverPort?.let { port ->
            SseProxyPortFile.deleteIfMatches(applicationContext, port, helperToken)
        }
        if (::sessionManager.isInitialized) {
            sessionManager.close()
        }
        if (::keepalive.isInitialized) {
            keepalive.release()
        }
        serviceScope.cancel()
        super.onDestroy()
    }

    private fun startTcpServer() {
        val job = serviceScope.launch(Dispatchers.IO, start = CoroutineStart.LAZY) {
            try {
                val server = ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"))
                serverSocket = server
                serverPort = server.localPort
                if (destroying) {
                    server.close()
                    return@launch
                }
                publishEndpoint(server.localPort)
                Log.i(TAG, "TCP 服务监听已就绪：result=ready")
                while (!server.isClosed) {
                    handleClientSocket(server.accept())
                }
            } catch (error: Exception) {
                if (serverSocket?.isClosed != true) {
                    failService("helper TCP 服务异常", error)
                }
            }
        }
        serverJob = job
        job.invokeOnCompletion {
            if (serverJob === job) serverJob = null
        }
        job.start()
    }

    private fun publishEndpoint(port: Int) {
        synchronized(endpointPublishJobs) {
            if (destroying) return
            val job = serviceScope.launch(Dispatchers.IO, start = CoroutineStart.LAZY) {
                if (destroying) return@launch
                if (!SseProxyPortFile.write(applicationContext, port, helperToken) && !destroying) {
                    failService("更新 helper 端点失败", null)
                }
            }
            endpointPublishJobs.add(job)
            job.invokeOnCompletion { endpointPublishJobs.remove(job) }
            job.start()
        }
    }

    private fun handleClientSocket(socket: Socket) {
        serviceScope.launch(Dispatchers.IO) {
            var commandDescription = "unknown"
            var commandStarted = false
            try {
                socket.soTimeout = INITIAL_COMMAND_READ_TIMEOUT_MS
                val input = socket.getInputStream()
                val output = socket.getOutputStream()
                val commandJson = SseSocketCodec.read(input) ?: return@launch
                val request = JSONObject(commandJson)
                val command = authorizeHelperCommand(request, helperToken)
                commandDescription = command.action
                // 仅完整解析且受支持的命令可以将 startup lease 转入正常
                // command/session 引用计数。
                beginCommand()
                commandStarted = true
                dispatchSseProxyCommand(
                    command,
                    socket,
                    input,
                    output,
                    sessionManager,
                    ::readSocketUntilClose,
                )
            } catch (error: Exception) {
                Log.e(TAG, "处理客户端 socket 失败：result=error, action=$commandDescription")
            } finally {
                try {
                    socket.close()
                } catch (_error: Exception) {
                    Log.w(TAG, "关闭客户端 socket 失败：result=error")
                }
                if (commandStarted) {
                    endCommand()
                }
            }
        }
    }

    private fun readSocketUntilClose(
        socket: Socket,
        input: java.io.InputStream,
        lease: SseSocketLease
    ) {
        try {
            socket.soTimeout = STREAM_SOCKET_READ_TIMEOUT_MS
            val buffer = ByteArray(1024)
            while (input.read(buffer) != -1) {
                // 维持连接，直到客户端主动断开。
            }
        } catch (error: Exception) {
            Log.d(TAG, "客户端 socket 读取结束：result=closed")
        } finally {
            sessionManager.onSocketDisconnected(lease)
            try {
                socket.close()
            } catch (_error: Exception) {
                Log.w(TAG, "关闭已断开 socket 失败：result=error")
            }
        }
    }

    private fun updateLocks() {
        if (!::keepalive.isInitialized || !::sessionManager.isInitialized) {
            return
        }
        val runningSessions = sessionManager.runningSessionCount()
        keepalive.update(runningSessions > 0)
        lifecycleLedger.sessionsChanged(runningSessions)?.let(::scheduleIdleStop)
        checkSelfTermination()
    }

    @Synchronized
    private fun beginCommand() {
        lifecycleLedger.commandStarted()
    }

    @Synchronized
    private fun endCommand() {
        lifecycleLedger.commandFinished()?.let(::scheduleIdleStop)
    }

    private fun scheduleIdleStop(token: SseProxyLifecycleLedger.IdleStopToken) {
        serviceScope.launch(Dispatchers.Main) {
            delay(IDLE_STOP_DELAY_MS)
            stopIfStillIdle(token)
        }
    }

    private fun scheduleStartupIdleStop(token: SseProxyLifecycleLedger.StartupIdleStopToken) {
        serviceScope.launch(Dispatchers.Main) {
            delay(STARTUP_IDLE_TIMEOUT_MS)
            stopIfStillStartupIdle(token)
        }
    }

    @Synchronized
    private fun stopIfStillIdle(token: SseProxyLifecycleLedger.IdleStopToken) {
        if (!lifecycleLedger.consumeIdleStop(token)) {
            return
        }
        Log.i(TAG, "helper 引用归零，停止前台服务 startId=${token.startId}")
        removeForeground()
        stopSelfResult(token.startId)
    }

    @Synchronized
    private fun stopIfStillStartupIdle(token: SseProxyLifecycleLedger.StartupIdleStopToken) {
        if (!lifecycleLedger.consumeStartupIdleStop(token)) {
            return
        }
        Log.i(TAG, "helper 启动后未收到合法命令，停止前台服务 startId=${token.startId}")
        removeForeground()
        stopSelfResult(token.startId)
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
        isTaskRemoved = true
        checkSelfTermination()
    }

    @Synchronized
    private fun checkSelfTermination() {
        if (!isTaskRemoved || !::sessionManager.isInitialized || lifecycleLedger.referenceCount() != 0) {
            return
        }
        Log.i(TAG, "任务已移除且没有运行中的会话，停止服务")
        removeForeground()
        stopSelf()
    }

    @Synchronized
    private fun failService(reason: String, error: Throwable?) {
        Log.e(TAG, "$reason：result=error")
        try {
            serverSocket?.close()
        } catch (_closeError: Exception) {
            Log.w(TAG, "关闭异常 helper TCP 服务失败：result=error")
        } finally {
            serverSocket = null
        }
        if (::sessionManager.isInitialized) {
            sessionManager.close()
        }
        if (::keepalive.isInitialized) {
            keepalive.release()
        }
        removeForeground()
        stopSelf()
    }

    private fun removeForeground() {
        if (!foregroundPromoted) {
            return
        }
        foreground.remove()
        foregroundPromoted = false
    }

    private fun isAppInForeground(): Boolean {
        val manager = getSystemService(Context.ACTIVITY_SERVICE)
            as? android.app.ActivityManager ?: return false
        val processes = manager.runningAppProcesses ?: return false
        return processes.any {
            it.importance == android.app.ActivityManager.RunningAppProcessInfo.IMPORTANCE_FOREGROUND &&
                it.processName == packageName
        }
    }
}
