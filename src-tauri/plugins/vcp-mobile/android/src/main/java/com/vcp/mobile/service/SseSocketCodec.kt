package com.vcp.mobile.service

import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer

/** Rust 客户端与 helper 服务共用的长度前缀 UTF-8 帧协议。 */
object SseSocketCodec {
    private const val MAX_FRAME_BYTES = 10 * 1024 * 1024

    fun write(output: OutputStream, json: String) {
        val bytes = json.toByteArray(Charsets.UTF_8)
        require(bytes.isNotEmpty() && bytes.size <= MAX_FRAME_BYTES) {
            "helper 帧超过 10MB 限制"
        }
        output.write(ByteBuffer.allocate(4 + bytes.size).putInt(bytes.size).put(bytes).array())
        output.flush()
    }

    fun read(input: InputStream): String? {
        val lengthBytes = ByteArray(4)
        if (!readFully(input, lengthBytes)) {
            return null
        }
        val length = ByteBuffer.wrap(lengthBytes).int
        if (length <= 0 || length > MAX_FRAME_BYTES) {
            return null
        }
        val data = ByteArray(length)
        if (!readFully(input, data)) {
            return null
        }
        return String(data, Charsets.UTF_8)
    }

    private fun readFully(input: InputStream, target: ByteArray): Boolean {
        var offset = 0
        while (offset < target.size) {
            val count = input.read(target, offset, target.size - offset)
            if (count < 0) {
                return false
            }
            if (count == 0) {
                continue
            }
            offset += count
        }
        return true
    }
}
