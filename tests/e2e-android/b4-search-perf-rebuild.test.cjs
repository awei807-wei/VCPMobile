"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  assertCdpBinding,
  getVerifiedCdpPid,
  rebuildIndex,
} = require("./b4-search-perf-rebuild.cjs");

function runtimeFor(pid, invoke) {
  return {
    cdp: {
      pid,
      boundPid: pid,
      socketName: `webview_devtools_remote_${pid}`,
      socketBindingVerified: true,
      cdp: { invoke, ws: { readyState: 1 } },
    },
  };
}

function rss(pid, bytes) {
  return {
    available: true,
    rssAvailable: true,
    metric: "rss",
    bytes,
    pid,
  };
}

test("重建 baseline/during/after 全程传递 verified CDP expected PID", async () => {
  const pid = 4_321;
  const memoryCalls = [];
  const runtime = runtimeFor(pid, async () => ({ durationMs: 2, indexedCount: 3 }));
  const result = await rebuildIndex(runtime, {
    readMemory: (options) => {
      memoryCalls.push(options);
      const bytes = memoryCalls.length === 2 ? 110 : 100 + memoryCalls.length;
      return rss(options.pid, bytes);
    },
    setInterval: () => "timer",
    clearInterval: () => {},
  });

  assert.equal(getVerifiedCdpPid(runtime), pid);
  assert.equal(result.expectedPid, pid);
  assert.deepEqual(
    memoryCalls.map((options) => [options.requireRss, options.pid]),
    [
      [true, pid],
      [true, pid],
      [true, pid],
    ],
  );
  assert.deepEqual(
    result.memorySamples.map((sample) => [sample.phase, sample.pid]),
    [
      ["before", pid],
      ["during", pid],
      ["after", pid],
    ],
  );
  assert.equal(result.memoryPeak.phase, "during");
});

test("verified、boundPid、socketName 任一不符时拒绝 RSS binding", () => {
  for (const mutate of [
    (runtime) => {
      runtime.cdp.socketBindingVerified = false;
    },
    (runtime) => {
      runtime.cdp.boundPid += 1;
    },
    (runtime) => {
      runtime.cdp.socketName = "webview_devtools_remote_other";
    },
  ]) {
    const runtime = runtimeFor(4_321, async () => ({ durationMs: 1 }));
    mutate(runtime);
    assert.throws(
      () => getVerifiedCdpPid(runtime),
      /无法从当前已验证 CDP binding 确定唯一 PID/,
    );
  }
});

test("CDP WebSocket 缺失、null 或非 OPEN 状态时严格拒绝", () => {
  for (const readyState of [undefined, null, 0, 2, 3]) {
    const runtime = runtimeFor(4_321, async () => ({ durationMs: 1 }));
    if (readyState === undefined) delete runtime.cdp.cdp.ws;
    else if (readyState === null) runtime.cdp.cdp.ws = null;
    else runtime.cdp.cdp.ws.readyState = readyState;
    assert.throws(
      () => assertCdpBinding(runtime, 4_321),
      /CDP binding|WebSocket/,
    );
  }
});

test("采样期间 CDP 对象、调用函数、WebSocket 状态或整套 binding 漂移时 fail-closed", async () => {
  const mutations = [
    (runtime) => {
      runtime.cdp.cdp = { ...runtime.cdp.cdp };
    },
    (runtime) => {
      runtime.cdp.cdp.invoke = async () => ({ durationMs: 1 });
    },
    (runtime) => {
      runtime.cdp.cdp.ws.readyState = 2;
    },
    (runtime) => {
      runtime.cdp.cdp.ws.readyState = 3;
    },
    (runtime) => {
      const replacementPid = 4_322;
      runtime.cdp.pid = replacementPid;
      runtime.cdp.boundPid = replacementPid;
      runtime.cdp.socketName = "webview_devtools_remote_" + replacementPid;
    },
  ];
  for (const mutate of mutations) {
    const pid = 4_321;
    const runtime = runtimeFor(pid, async () => ({ durationMs: 1 }));
    let sampleCalls = 0;
    const result = await rebuildIndex(runtime, {
      readMemory: (options) => {
        sampleCalls += 1;
        if (sampleCalls === 1) mutate(runtime);
        return rss(options.pid, 100);
      },
      setInterval: () => null,
      clearInterval: () => {},
    });
    assert.equal(result.memorySamples[0].available, false);
    assert.ok(
      result.memorySamplingFailures.some((failure) =>
        /漂移|未保持打开|唯一 PID/.test(failure.message),
      ),
    );
  }
});

test("严格采样不重新 pidof，PID-specific proc 读数拒绝漂移结果", async () => {
  const pid = 4_321;
  const calls = [];
  const runtime = runtimeFor(pid, async () => ({ durationMs: 1 }));
  const result = await rebuildIndex(runtime, {
    readMemory: (options) => {
      calls.push(options);
      return rss(options.pid, 100);
    },
    setInterval: () => null,
    clearInterval: () => {},
  });
  assert.equal(result.memorySamplingFailures.length, 0);
  assert.ok(calls.every((options) => options.pid === pid));
});

test("during 采样 PID 不一致时重建样本 fail-closed", async () => {
  const pid = 4_321;
  let sampleIndex = 0;
  const runtime = runtimeFor(pid, async () => ({ durationMs: 1 }));
  const result = await rebuildIndex(runtime, {
    readMemory: (options) => {
      sampleIndex += 1;
      return rss(sampleIndex === 2 ? pid + 1 : options.pid, 100);
    },
    setInterval: () => null,
    clearInterval: () => {},
  });
  assert.equal(result.memorySamples[1].available, false);
  assert.equal(result.memorySamples[1].pid, pid);
  assert.ok(
    result.memorySamplingFailures.some((failure) =>
      /PID 不匹配/.test(failure.message),
    ),
  );
});
