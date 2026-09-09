"use strict";

const http = require("node:http");
const { SSE_PATHS, codedError, validatePort } = require("./helper-stream-e2e-support.cjs");

function endpointPath(url) {
  return SSE_PATHS.includes(url) ? url : null;
}

function safeRequestContract(request, url, body) {
  let parsed = null;
  try {
    parsed = JSON.parse(body);
  } catch {
    parsed = null;
  }
  const stream = typeof parsed?.stream === "boolean" ? parsed.stream : null;
  return {
    method: request.method === "POST" ? "POST" : "other",
    path: endpointPath(url) || "other",
    headers: {
      authorizationPresent: typeof request.headers.authorization === "string" &&
        request.headers.authorization.length > 0,
      contentTypePresent: typeof request.headers["content-type"] === "string" &&
        request.headers["content-type"].length > 0,
      acceptSsePresent: typeof request.headers.accept === "string" &&
        request.headers.accept.includes("text/event-stream"),
    },
    body: {
      validJson: parsed !== null && typeof parsed === "object" && !Array.isArray(parsed),
      stream,
      requestIdPresent: typeof parsed?.requestId === "string",
      messagesArray: Array.isArray(parsed?.messages),
      messageCount: Array.isArray(parsed?.messages) ? parsed.messages.length : null,
      modelPresent: typeof parsed?.model === "string",
    },
  };
}

function waitForEvent(waiters, values, name, timeoutMs) {
  if (values.has(name)) return Promise.resolve(values.get(name));
  const timeout = Number.isFinite(timeoutMs) && timeoutMs > 0 ? timeoutMs : 15_000;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      const list = waiters.get(name) || [];
      waiters.set(name, list.filter((item) => item !== settle));
      reject(codedError("SSE_EVENT_TIMEOUT", "SSE 事件等待超时"));
    }, timeout);
    const settle = (value) => {
      clearTimeout(timer);
      resolve(value);
    };
    const list = waiters.get(name) || [];
    list.push(settle);
    waiters.set(name, list);
  });
}

function emitEvent(waiters, values, name, value) {
  values.set(name, value);
  const list = waiters.get(name) || [];
  waiters.delete(name);
  for (const settle of list) settle(value);
}

function sseChunk(label) {
  return `data: ${JSON.stringify({
    id: "android-helper-e2e",
    object: "chat.completion.chunk",
    choices: [{ index: 0, delta: { content: label }, finish_reason: null }],
  })}\n\n`;
}

function readRequestBody(request, maxBytes) {
  return new Promise((resolve, reject) => {
    let total = 0;
    const chunks = [];
    request.on("data", (chunk) => {
      total += chunk.length;
      if (total > maxBytes) {
        reject(codedError("SSE_BODY_TOO_LARGE", "SSE 请求正文超限"));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.once("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    request.once("error", () => reject(codedError("SSE_REQUEST_ERROR", "SSE 请求读取失败")));
  });
}

function createSessionController(waiters, eventValues) {
  const state = { active: null, bReleased: false };
  function markCancelled(session) {
    if (!session || session.ended || session.cancelled) return;
    session.cancelled = true;
    if (state.active === session) state.active = null;
    emitEvent(waiters, eventValues, "cancelled", true);
  }
  function sendB(session) {
    if (!session || session.ended || session.cancelled || session.bSent) return false;
    session.response.write(sseChunk("B"));
    session.bSent = true;
    emitEvent(waiters, eventValues, "b", true);
    return true;
  }
  function sendDone(session) {
    if (!session || session.ended || session.cancelled) return false;
    session.response.write("data: [DONE]\n\n");
    session.ended = true;
    session.response.end();
    if (state.active === session) state.active = null;
    emitEvent(waiters, eventValues, "done", true);
    return true;
  }
  return {
    state,
    markCancelled,
    sendB,
    sendDone,
    releaseB() {
      state.bReleased = true;
      return sendB(state.active);
    },
  };
}

function createRequestHandler(maxBytes, contracts, waiters, eventValues, controller) {
  async function handleRequest(request, response) {
    const url = typeof request.url === "string" ? request.url.split("?", 1)[0] : "";
    const body = await readRequestBody(request, maxBytes);
    const contract = safeRequestContract(request, url, body);
    contracts.push(contract);
    if (request.method !== "POST" || !endpointPath(url)) {
      response.writeHead(404, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: "not_found" }));
      return;
    }
    if (!contract.body.validJson || contract.body.stream !== true) {
      response.writeHead(400, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: "invalid_stream_contract" }));
      return;
    }
    response.writeHead(200, {
      "cache-control": "no-cache",
      connection: "keep-alive",
      "content-type": "text/event-stream; charset=utf-8",
    });
    const session = { response, ended: false, cancelled: false, bSent: false };
    controller.state.active = session;
    response.once("close", () => controller.markCancelled(session));
    response.once("error", () => controller.markCancelled(session));
    response.write(sseChunk("A"));
    emitEvent(waiters, eventValues, "a", true);
    if (controller.state.bReleased) controller.sendB(session);
    emitEvent(waiters, eventValues, "accepted", contract);
  }
  return (request, response) => {
    handleRequest(request, response).catch(() => {
      if (!response.headersSent) response.writeHead(500);
      response.end();
    });
  };
}

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.removeListener("error", reject);
      resolve();
    });
  });
}

function startLoopbackSseServer(options = {}) {
  const maxBytes = options.maxBodyBytes || 1_048_576;
  const contracts = [];
  const waiters = new Map();
  const eventValues = new Map();
  const controller = createSessionController(waiters, eventValues);
  const server = http.createServer(
    createRequestHandler(maxBytes, contracts, waiters, eventValues, controller),
  );
  return listen(server).then(() => {
    const port = validatePort(server.address()?.port);
    let closed = false;
    return {
      port,
      baseUrl: `http://127.0.0.1:${port}`,
      contracts,
      waitFor: (name, timeoutMs) => waitForEvent(waiters, eventValues, name, timeoutMs),
      releaseB: controller.releaseB,
      finish: () => controller.sendDone(controller.state.active),
      isCancelled: () => !controller.state.active && contracts.length > 0,
      snapshotContracts: () => contracts.map((contract) => structuredClone(contract)),
      async close() {
        if (closed) return;
        closed = true;
        if (controller.state.active && !controller.state.active.ended) {
          controller.state.active.response.destroy();
        }
        await new Promise((resolve) => server.close(resolve));
      },
    };
  });
}

module.exports = {
  safeRequestContract,
  sseChunk,
  startLoopbackSseServer,
};
