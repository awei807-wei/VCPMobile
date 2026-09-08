package com.vcp.mobile.service

import android.content.Context
import android.util.Log
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import org.json.JSONObject

/** helper 端口文件的原子职责，失败由服务决定是否停止。 */
internal object SseProxyPortFile {
    private const val TAG = "VcpSseProxyPort"
    private const val FILE_NAME = "sse_helper.port"
    private val fileLock = Any()
    private val fieldPattern = Regex("\"(version|port|token)\"\\s*:\\s*(?:\"([^\"\\\\]*)\"|(-?\\d+))")
    private val delimitersPattern = Regex("[\\s{},]")

    data class Endpoint(val port: Int, val token: String)

    fun write(context: Context, port: Int, token: String): Boolean {
        if (port !in 1..65535 || token.isBlank()) return false
        return synchronized(fileLock) {
            val target = File(context.cacheDir, FILE_NAME)
            val temporary = File.createTempFile("$FILE_NAME.", ".tmp", context.cacheDir)
            try {
                val payload = "{\"version\":1,\"port\":$port,\"token\":${quote(token)}}"
                temporary.outputStream().use { output ->
                    output.write(payload.toByteArray(Charsets.UTF_8))
                    output.fd.sync()
                }
                Files.move(
                    temporary.toPath(),
                    target.toPath(),
                    StandardCopyOption.ATOMIC_MOVE,
                    StandardCopyOption.REPLACE_EXISTING,
                )
                true
            } catch (error: Exception) {
                Log.e(TAG, "写入 helper 端点失败：result=error")
                false
            } finally {
                temporary.delete()
            }
        }
    }

    fun read(context: Context): Endpoint? {
        return try {
            parse(File(context.cacheDir, FILE_NAME).readText())
        } catch (_: Exception) {
            null
        }
    }

    internal fun parse(content: String): Endpoint {
        val fields = linkedMapOf<String, String>()
        fieldPattern.findAll(content).forEach { match ->
            val name = match.groupValues[1]
            require(fields.put(name, match.groupValues[2].ifEmpty { match.groupValues[3] }) == null) {
                "helper endpoint 字段重复"
            }
        }
        require(delimitersPattern.replace(fieldPattern.replace(content, ""), "").isEmpty()) {
            "helper endpoint JSON 结构无效"
        }
        val version = fields["version"]?.toIntOrNull() ?: -1
        require(version == 1) { "helper endpoint version 无效" }
        val port = fields["port"]?.toIntOrNull() ?: -1
        require(port in 1..65535) { "helper endpoint port 无效" }
        val token = fields["token"]
        require(!token.isNullOrBlank()) { "helper endpoint token 缺失" }
        return Endpoint(port, token)
    }

    private fun quote(value: String): String {
        val escaped = buildString(value.length + 2) {
            append('"')
            value.forEach { character ->
                when (character) {
                    '\\' -> append("\\\\")
                    '"' -> append("\\\"")
                    '\b' -> append("\\b")
                    '\u000C' -> append("\\f")
                    '\n' -> append("\\n")
                    '\r' -> append("\\r")
                    '\t' -> append("\\t")
                    else -> if (character < ' ') {
                        append("\\u%04x".format(character.code))
                    } else {
                        append(character)
                    }
                }
            }
            append('"')
        }
        return escaped
    }

    /** 仅当当前文件仍属于本 helper 实例时删除，避免旧实例清理新实例端点。 */
    fun deleteIfMatches(context: Context, port: Int, token: String): Boolean {
        if (port !in 1..65535 || token.isBlank()) return false
        return synchronized(fileLock) {
            val target = File(context.cacheDir, FILE_NAME)
            val endpoint = runCatching { parse(target.readText()) }.getOrNull()
            if (endpoint?.port != port || endpoint.token != token) return@synchronized false
            try {
                target.delete()
            } catch (error: Exception) {
                Log.w(TAG, "删除 helper 端点失败：result=error")
                false
            }
        }
    }
}
