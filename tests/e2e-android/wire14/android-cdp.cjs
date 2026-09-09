"use strict";

const net = require("node:net");
const {
  DEBUG_PACKAGE,
  ensureSingleDevice,
  runAdb,
} = require("../scripts/adb-env.cjs");

const EVENT_NAMES = [
  "vcp-sync-status",
  "vcp-sync-progress",
  "vcp-sync-completed",
  "vcp-log",
];
const DEFAULT_CDP_TIMEOUT_MS = 15_000;
const PID_SOCKET_PREFIX = "webview_devtools_remote_";

function resolveTimeoutMs(timeoutOrOptions) {
  const candidate =
    typeof timeoutOrOptions === "number"
      ? timeoutOrOptions
      : timeoutOrOptions?.timeoutMs ?? timeoutOrOptions?.timeout;
  return Number.isFinite(candidate) && candidate > 0
    ? Math.max(1, Math.floor(candidate))
    : DEFAULT_CDP_TIMEOUT_MS;
}

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

function packagePid(adb = runAdb) {
  const output = adb(["shell", "pidof", "-s", DEBUG_PACKAGE], {
    allowFailure: true,
  }).trim();
  const pid = Number(output.split(/\s+/)[0]);
  if (!Number.isSafeInteger(pid) || pid <= 0) {
    throw new Error("Android Debug 应用未运行");
  }
  return pid;
}

function pidSocketName(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) {
    throw codedError(
      "ANDROID_CDP_PID_UNAVAILABLE",
      "无法为 Android WebView CDP 确定有效进程 PID",
    );
  }
  return `${PID_SOCKET_PREFIX}${pid}`;
}

function forwardSocket(localPort, pid, adb = runAdb) {
  const socketName = pidSocketName(pid);
  try {
    adb(["forward", `tcp:${localPort}`, `localabstract:${socketName}`]);
  } catch (error) {
    throw codedError(
      "ANDROID_CDP_PID_SOCKET_FORWARD_FAILED",
      `无法绑定 Android WebView 当前 PID 的 CDP socket（pid=${pid}）`,
      error,
    );
  }
  return { socketName, boundPid: pid, verified: true };
}

function codedError(code, message, cause) {
  const error = new Error(message, cause ? { cause } : undefined);
  error.code = code;
  return error;
}

async function fetchJson(url, timeoutOrOptions, dependencies = {}) {
  const timeoutMs = resolveTimeoutMs(timeoutOrOptions);
  const fetchImpl = dependencies.fetch || fetch;
  const setTimer = dependencies.setTimeout || setTimeout;
  const clearTimer = dependencies.clearTimeout || clearTimeout;
  const controller = new AbortController();
  let timer = null;
  try {
    timer = setTimer(() => controller.abort(), timeoutMs);
    const response = await fetchImpl(url, { signal: controller.signal });
    if (!response.ok) throw new Error(`CDP 端点返回 HTTP ${response.status}`);
    return await response.json();
  } catch (error) {
    if (controller.signal.aborted) {
      throw codedError("CDP_JSON_TIMEOUT", `CDP JSON 请求超时（${timeoutMs}ms）`, error);
    }
    throw error;
  } finally {
    if (timer !== null) clearTimer(timer);
  }
}

function normalizeWsUrl(rawUrl, localPort) {
  const url = new URL(rawUrl);
  url.hostname = "127.0.0.1";
  url.port = String(localPort);
  return url.toString();
}

async function findPage(localPort, timeoutOrOptions, dependencies = {}) {
  const targets = await fetchJson(
    `http://127.0.0.1:${localPort}/json/list`,
    timeoutOrOptions,
    dependencies,
  );
  const page = Array.isArray(targets)
    ? targets.find(
        (target) => target.type === "page" && target.webSocketDebuggerUrl,
      )
    : null;
  if (!page) throw new Error("Android WebView 没有可用的 CDP 页面");
  return {
    title: typeof page.title === "string" ? page.title.slice(0, 80) : "",
    url: typeof page.url === "string" ? page.url.slice(0, 160) : "",
    wsUrl: normalizeWsUrl(page.webSocketDebuggerUrl, localPort),
  };
}

class CdpClient {
  constructor(wsUrl, dependencies = {}) {
    this.wsUrl = wsUrl;
    this.WebSocketImpl = dependencies.WebSocket || WebSocket;
    this.ws = null;
    this.nextId = 0;
    this.pending = new Map();
  }

  async connect(timeoutOrOptions) {
    const timeoutMs = resolveTimeoutMs(timeoutOrOptions);
    const ws = new this.WebSocketImpl(this.wsUrl);
    this.ws = ws;
    await new Promise((resolve, reject) => {
      let settled = false;
      let timer = null;
      const onMessage = (event) => this.receive(event.data);
      const removeListeners = () => {
        ws.removeEventListener("message", onMessage);
        ws.removeEventListener("error", onError);
        ws.removeEventListener("close", onClose);
        ws.removeEventListener("open", onOpen);
      };
      const fail = (error) => {
        if (settled) return;
        settled = true;
        if (timer !== null) clearTimeout(timer);
        removeListeners();
        if (this.ws === ws) this.ws = null;
        try {
          ws.close();
        } catch {}
        reject(error);
      };
      const onOpen = () => {
        if (settled) return;
        settled = true;
        if (timer !== null) clearTimeout(timer);
        ws.removeEventListener("open", onOpen);
        resolve();
      };
      const onError = () => {
        this.rejectPending(new Error("CDP WebSocket 错误"));
        fail(new Error("无法连接 Android WebView CDP"));
      };
      const onClose = () => {
        this.rejectPending(new Error("CDP WebSocket 已关闭"));
        fail(new Error("无法连接 Android WebView CDP：WebSocket 已关闭"));
      };
      ws.addEventListener("message", onMessage);
      ws.addEventListener("error", onError);
      ws.addEventListener("close", onClose);
      ws.addEventListener("open", onOpen);
      timer = setTimeout(
        () =>
          fail(
            codedError(
              "CDP_WEBSOCKET_CONNECT_TIMEOUT",
              `Android WebView CDP 握手超时（${timeoutMs}ms）`,
            ),
          ),
        timeoutMs,
      );
    });
  }

  receive(rawMessage) {
    let message;
    try {
      message = JSON.parse(String(rawMessage));
    } catch {
      return;
    }
    if (!Number.isSafeInteger(message.id)) return;
    const request = this.pending.get(message.id);
    if (!request) return;
    this.pending.delete(message.id);
    if (message.error) {
      request.reject(
        new Error(`CDP 调用失败：${message.error.code || "unknown"}`),
      );
    } else {
      request.resolve(message.result);
    }
  }

  rejectPending(error) {
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
  }

  call(method, params = {}, timeoutOrOptions) {
    const id = ++this.nextId;
    const timeoutMs = resolveTimeoutMs(timeoutOrOptions);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP 调用超时：${method}`));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate(expression, timeoutOrOptions) {
    const result = await this.call("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
      userGesture: true,
    }, timeoutOrOptions);
    if (result?.exceptionDetails) {
      throw new Error("WebView JavaScript 执行失败");
    }
    return result?.result?.value;
  }

  async invoke(command, args = {}, timeoutOrOptions) {
    const expression = `(async () => {
      const internals = window.__TAURI_INTERNALS__;
      if (!internals || typeof internals.invoke !== 'function') {
        throw new Error('Tauri IPC 不可用');
      }
      return await internals.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)});
    })()`;
    return this.evaluate(expression, timeoutOrOptions);
  }

  async installEventBuffer() {
    const names = JSON.stringify(EVENT_NAMES);
    const expression = `(async () => {
      const internals = window.__TAURI_INTERNALS__;
      if (!internals || typeof internals.invoke !== 'function' ||
          typeof internals.transformCallback !== 'function') {
        throw new Error('Tauri 事件 IPC 不可用');
      }
      const state = window.__wire14E2E || { events: [], listeners: [] };
      state.events = [];
      state.listeners = [];
      window.__wire14E2E = state;
      for (const name of ${names}) {
        const callbackId = internals.transformCallback((event) => {
          const payload = event && typeof event === 'object' &&
            Object.prototype.hasOwnProperty.call(event, 'payload')
            ? event.payload : event;
          state.events.push({ name, payload });
          if (state.events.length > 1000) state.events.shift();
        });
        const eventId = await internals.invoke('plugin:event|listen', {
          event: name,
          target: { kind: 'Any' },
          handler: callbackId,
        });
        state.listeners.push({ name, eventId, callbackId });
      }
      return true;
    })()`;
    return this.evaluate(expression);
  }

  async readEvents() {
    return (
      (await this.evaluate(`(() => {
      const state = window.__wire14E2E;
      return state ? state.events.splice(0, state.events.length) : [];
    })()`)) || []
    );
  }

  async removeEventBuffer() {
    const expression = `(async () => {
      const state = window.__wire14E2E;
      const internals = window.__TAURI_INTERNALS__;
      if (!state || !internals) return true;
      for (const listener of state.listeners || []) {
        try {
          await internals.invoke('plugin:event|unlisten', {
            event: listener.name,
            eventId: listener.eventId,
          });
        } catch {}
        try { internals.unregisterCallback(listener.callbackId); } catch {}
      }
      delete window.__wire14E2E;
      return true;
    })()`;
    return this.evaluate(expression);
  }

  async close() {
    this.rejectPending(new Error("CDP 已关闭"));
    if (this.ws && this.ws.readyState === WebSocket.OPEN) this.ws.close();
    this.ws = null;
  }
}

async function connectAndroidCdp(timeoutOrOptions, dependencies = {}) {
  const timeoutMs = resolveTimeoutMs(timeoutOrOptions);
  const adb = dependencies.runAdb || runAdb;
  const ensureDevice = dependencies.ensureSingleDevice || ensureSingleDevice;
  const readPid = dependencies.packagePid || (() => packagePid(adb));
  const reservePort = dependencies.freePort || freePort;
  const forward =
    dependencies.forwardSocket || ((localPort, pid) => forwardSocket(localPort, pid, adb));
  const find = dependencies.findPage || ((localPort, timeout) =>
    findPage(localPort, timeout, dependencies));
  const device = await ensureDevice();
  const pid = await readPid();
  const localPort = await reservePort();
  try {
    const binding = await forward(localPort, pid);
    const expectedSocketName = pidSocketName(pid);
    if (
      !binding ||
      binding.verified !== true ||
      binding.boundPid !== pid ||
      binding.socketName !== expectedSocketName
    ) {
      throw codedError(
        "ANDROID_CDP_PID_SOCKET_UNVERIFIED",
        `Android WebView CDP 未证明绑定当前 PID-specific socket（pid=${pid}）`,
      );
    }
    const socketName = binding.socketName;
    const page = await find(localPort, timeoutMs);
    const CdpClientImpl = dependencies.CdpClient || CdpClient;
    const cdp = new CdpClientImpl(page.wsUrl);
    await cdp.connect(timeoutMs);
    return {
      deviceSerial: device.serial,
      pid,
      boundPid: binding.boundPid,
      localPort,
      socketName,
      socketBindingVerified: binding.verified,
      page,
      cdp,
      async close() {
        await cdp.removeEventBuffer().catch(() => {});
        await cdp.close();
        adb(["forward", "--remove", `tcp:${localPort}`], {
          allowFailure: true,
        });
      },
    };
  } catch (error) {
    adb(["forward", "--remove", `tcp:${localPort}`], { allowFailure: true });
    throw error;
  }
}

module.exports = {
  CdpClient,
  DEFAULT_CDP_TIMEOUT_MS,
  EVENT_NAMES,
  connectAndroidCdp,
  fetchJson,
  forwardSocket,
  findPage,
  pidSocketName,
  resolveTimeoutMs,
};
