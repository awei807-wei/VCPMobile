package com.vcp.mobile.service

import java.io.InputStream
import java.io.OutputStream
import java.net.Socket

/** 将已完成授权的 helper 命令分派给会话管理器。 */
internal fun dispatchSseProxyCommand(
    command: SseProxyCommand,
    socket: Socket,
    input: InputStream,
    output: OutputStream,
    sessionManager: SseSessionManager,
    readSocketUntilClose: (Socket, InputStream, SseSocketLease) -> Unit,
) {
    when (command.action) {
        "start" -> {
            val lease = sessionManager.start(command, output)
            readSocketUntilClose(socket, input, lease)
        }

        "resume" -> {
            val lease = sessionManager.resume(
                command.key,
                command.startIndex,
                output,
                command.expectedGeneration
                    ?: throw IllegalArgumentException("resume 命令缺少 expectedGeneration"),
            )
            if (lease != null) readSocketUntilClose(socket, input, lease)
        }

        "prepare_resume" -> sessionManager.prepareResume(
            command.key,
            command.expectedGeneration
                ?: throw IllegalArgumentException("prepare_resume 命令缺少 expectedGeneration"),
            output,
        )

        "cancel_resume" -> sessionManager.cancelResume(
            command.key,
            command.expectedGeneration
                ?: throw IllegalArgumentException("cancel_resume 命令缺少 expectedGeneration"),
        )

        "query" -> sessionManager.query(command.key, output)
        "stop" -> sessionManager.stop(
            command.key,
            command.expectedGeneration
                ?: throw IllegalArgumentException("stop 命令缺少 expectedGeneration"),
        )

        else -> throw IllegalArgumentException("未知 helper action：${command.action}")
    }
}
