package com.vcp.mobile.service

import org.json.JSONObject

/** 在任何命令解析或会话分发前校验本 helper 实例凭据。 */
internal fun authorizeHelperCommand(
    request: JSONObject,
    helperToken: String,
    parse: (JSONObject) -> SseProxyCommand = { value -> SseProxyCommand.parse(value) },
): SseProxyCommand {
    val suppliedToken = request.opt("token") as? String
    return authorizeHelperToken(suppliedToken, helperToken, request, parse)
}

internal fun authorizeHelperToken(
    suppliedToken: String?,
    helperToken: String,
    request: JSONObject,
    parse: (JSONObject) -> SseProxyCommand,
): SseProxyCommand {
    if (!SseProxyAuth.constantTimeEquals(helperToken, suppliedToken)) {
        throw IllegalArgumentException("helper 身份验证失败")
    }
    return parse(request)
}
