"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  CdpClient,
  connectAndroidCdp,
  fetchJson,
} = require("./android-cdp.cjs");

const nativeClearTimeout = clearTimeout;

function clearTimerCounter() {
  const state = { count: 0 };
  return {
    state,
    clearTimeout(timer) {
      state.count += 1;
      nativeClearTimeout(timer);
    },
  };
}

function hangingFetch() {
  return (_url, { signal }) =>
    new Promise((resolve, reject) => {
      signal.addEventListener(
        "abort",
        () => {
          const error = new Error("fetch aborted");
          error.name = "AbortError";
          reject(error);
        },
        { once: true },
      );
    });
}

class HangingWebSocket {
  static instances = [];

  constructor(url) {
    this.url = url;
    this.readyState = 0;
    this.listeners = new Map();
    this.closed = false;
    HangingWebSocket.instances.push(this);
  }

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) || new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type, listener) {
    this.listeners.get(type)?.delete(listener);
  }

  close() {
    this.closed = true;
    this.readyState = 3;
  }

  listenerCount() {
    let count = 0;
    for (const listeners of this.listeners.values()) count += listeners.size;
    return count;
  }
}

test("fetchJson 超时会 abort 请求、清理定时器并返回稳定错误码", async () => {
  const timer = clearTimerCounter();
  let aborted = false;
  await assert.rejects(
    fetchJson("http://127.0.0.1:1/json/list", { timeoutMs: 10 }, {
      fetch: (_url, { signal }) => {
        signal.addEventListener("abort", () => {
          aborted = true;
        }, { once: true });
        return hangingFetch()(_url, { signal });
      },
      clearTimeout: timer.clearTimeout,
    }),
    (error) => {
      assert.equal(error.code, "CDP_JSON_TIMEOUT");
      return true;
    },
  );
  assert.equal(aborted, true);
  assert.equal(timer.state.count, 1);
});

test("json/list 请求超时仍会移除 ADB forward", async () => {
  const timer = clearTimerCounter();
  const adbCalls = [];
  await assert.rejects(
    connectAndroidCdp(10, {
      ensureSingleDevice: () => ({ serial: "test-device" }),
      packagePid: () => 4321,
      freePort: async () => 4567,
      runAdb: (args, options) => {
        adbCalls.push({ args, options });
        return "";
      },
      fetch: hangingFetch(),
      clearTimeout: timer.clearTimeout,
    }),
    (error) => {
      assert.equal(error.code, "CDP_JSON_TIMEOUT");
      return true;
    },
  );
  assert.equal(timer.state.count, 1);
  assert.deepEqual(adbCalls[0].args, [
    "forward",
    "tcp:4567",
    "localabstract:webview_devtools_remote_4321",
  ]);
  assert.deepEqual(adbCalls.at(-1), {
    args: ["forward", "--remove", "tcp:4567"],
    options: { allowFailure: true },
  });
});

test("verified、boundPid 或 socketName 任一异常都会拒绝 CDP binding", async () => {
  for (const binding of [
    {
      verified: false,
      boundPid: 4321,
      socketName: "webview_devtools_remote_4321",
    },
    {
      verified: true,
      boundPid: 4322,
      socketName: "webview_devtools_remote_4321",
    },
    {
      verified: true,
      boundPid: 4321,
      socketName: "webview_devtools_remote_other",
    },
  ]) {
    const adbCalls = [];
    await assert.rejects(
      connectAndroidCdp(10, {
        ensureSingleDevice: () => ({ serial: "test-device" }),
        packagePid: () => 4321,
        freePort: async () => 4569,
        forwardSocket: async () => binding,
        runAdb: (args, options) => {
          adbCalls.push({ args, options });
          return "";
        },
      }),
      (error) => {
        assert.equal(error.code, "ANDROID_CDP_PID_SOCKET_UNVERIFIED");
        return true;
      },
    );
    assert.deepEqual(adbCalls.at(-1), {
      args: ["forward", "--remove", "tcp:4569"],
      options: { allowFailure: true },
    });
  }
});

test("WebSocket 握手超时会关闭 socket、移除监听并返回稳定错误码", async () => {
  const client = new CdpClient("ws://127.0.0.1/devtools/page", {
    WebSocket: HangingWebSocket,
  });
  await assert.rejects(
    client.connect({ timeoutMs: 10 }),
    (error) => {
      assert.equal(error.code, "CDP_WEBSOCKET_CONNECT_TIMEOUT");
      return true;
    },
  );
  const socket = HangingWebSocket.instances.at(-1);
  assert.equal(socket.closed, true);
  assert.equal(socket.listenerCount(), 0);
  assert.equal(client.ws, null);
});

test("WebSocket 握手超时经 connectAndroidCdp 触发 forward 清理", async () => {
  const adbCalls = [];
  class TestCdpClient extends CdpClient {
    constructor(url) {
      super(url, { WebSocket: HangingWebSocket });
    }
  }
  await assert.rejects(
    connectAndroidCdp(10, {
      ensureSingleDevice: () => ({ serial: "test-device" }),
      packagePid: () => 4321,
      freePort: async () => 4568,
      runAdb: (args, options) => {
        adbCalls.push({ args, options });
        return "";
      },
      fetch: async () => ({
        ok: true,
        json: async () => [
          {
            type: "page",
            webSocketDebuggerUrl: "ws://device/devtools/page",
          },
        ],
      }),
      CdpClient: TestCdpClient,
    }),
    (error) => {
      assert.equal(error.code, "CDP_WEBSOCKET_CONNECT_TIMEOUT");
      return true;
    },
  );
  const socket = HangingWebSocket.instances.at(-1);
  assert.equal(socket.closed, true);
  assert.equal(socket.listenerCount(), 0);
  assert.deepEqual(adbCalls.at(-1), {
    args: ["forward", "--remove", "tcp:4568"],
    options: { allowFailure: true },
  });
});
