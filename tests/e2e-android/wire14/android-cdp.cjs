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

function packagePid() {
  const output = runAdb(["shell", "pidof", "-s", DEBUG_PACKAGE], {
    allowFailure: true,
  }).trim();
  const pid = Number(output.split(/\s+/)[0]);
  if (!Number.isSafeInteger(pid) || pid <= 0) {
    throw new Error("Android Debug 应用未运行");
  }
  return pid;
}

function forwardSocket(localPort, pid) {
  const candidates = [
    `webview_devtools_remote_${pid}`,
    "webview_devtools_remote",
  ];
  let lastError = null;
  for (const socketName of candidates) {
    try {
      runAdb(["forward", `tcp:${localPort}`, `localabstract:${socketName}`]);
      return socketName;
    } catch (error) {
      lastError = error;
    }
  }
  throw new Error(
    `无法建立 Android WebView 调试转发（${lastError?.code || "未知原因"}）`,
  );
}

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`CDP 端点返回 HTTP ${response.status}`);
  return response.json();
}

function normalizeWsUrl(rawUrl, localPort) {
  const url = new URL(rawUrl);
  url.hostname = "127.0.0.1";
  url.port = String(localPort);
  return url.toString();
}

async function findPage(localPort) {
  const targets = await fetchJson(`http://127.0.0.1:${localPort}/json/list`);
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
  constructor(wsUrl) {
    this.wsUrl = wsUrl;
    this.ws = null;
    this.nextId = 0;
    this.pending = new Map();
  }

  async connect() {
    const ws = new WebSocket(this.wsUrl);
    this.ws = ws;
    ws.addEventListener("message", (event) => this.receive(event.data));
    ws.addEventListener("error", () =>
      this.rejectPending(new Error("CDP WebSocket 错误")),
    );
    ws.addEventListener("close", () =>
      this.rejectPending(new Error("CDP WebSocket 已关闭")),
    );
    await new Promise((resolve, reject) => {
      const onOpen = () => {
        ws.removeEventListener("open", onOpen);
        ws.removeEventListener("error", onError);
        resolve();
      };
      const onError = () => {
        ws.removeEventListener("open", onOpen);
        ws.removeEventListener("error", onError);
        reject(new Error("无法连接 Android WebView CDP"));
      };
      ws.addEventListener("open", onOpen);
      ws.addEventListener("error", onError);
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

  call(method, params = {}) {
    const id = ++this.nextId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP 调用超时：${method}`));
      }, 15_000);
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

  async evaluate(expression) {
    const result = await this.call("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
      userGesture: true,
    });
    if (result?.exceptionDetails) {
      throw new Error("WebView JavaScript 执行失败");
    }
    return result?.result?.value;
  }

  async invoke(command, args = {}) {
    const expression = `(async () => {
      const internals = window.__TAURI_INTERNALS__;
      if (!internals || typeof internals.invoke !== 'function') {
        throw new Error('Tauri IPC 不可用');
      }
      return await internals.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)});
    })()`;
    return this.evaluate(expression);
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

async function connectAndroidCdp() {
  const device = ensureSingleDevice();
  const pid = packagePid();
  const localPort = await freePort();
  const socketName = forwardSocket(localPort, pid);
  try {
    const page = await findPage(localPort);
    const cdp = new CdpClient(page.wsUrl);
    await cdp.connect();
    return {
      deviceSerial: device.serial,
      pid,
      localPort,
      socketName,
      page,
      cdp,
      async close() {
        await cdp.removeEventBuffer().catch(() => {});
        await cdp.close();
        runAdb(["forward", "--remove", `tcp:${localPort}`], {
          allowFailure: true,
        });
      },
    };
  } catch (error) {
    runAdb(["forward", "--remove", `tcp:${localPort}`], { allowFailure: true });
    throw error;
  }
}

module.exports = {
  CdpClient,
  EVENT_NAMES,
  connectAndroidCdp,
};
