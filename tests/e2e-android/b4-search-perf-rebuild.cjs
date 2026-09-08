"use strict";

const {
  errorText,
  findMemoryPeak,
  REBUILD_CDP_TIMEOUT_MS,
  readAndroidMemory,
  roundMs,
  unavailableReading,
} = require("./b4-search-perf-metrics.cjs");
const { isRssReading } = require("./b4-search-perf-memory.cjs");

const MEMORY_SAMPLE_INTERVAL_MS = 250;
const PID_SOCKET_PREFIX = "webview_devtools_remote_";

function isPid(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function getVerifiedCdpPid(runtime) {
  const binding = runtime?.cdp;
  const pid = binding?.pid;
  const boundPid = binding?.boundPid;
  const socketMatches =
    binding?.socketName === `${PID_SOCKET_PREFIX}${pid}`;
  if (
    binding?.socketBindingVerified !== true ||
    !isPid(pid) ||
    !isPid(boundPid) ||
    pid !== boundPid ||
    !socketMatches
  ) {
    throw new Error(
      "无法从当前已验证 CDP binding 确定唯一 PID；拒绝 RSS 采样。",
    );
  }
  return pid;
}

function assertCdpBinding(runtime, expectedPid, previous = null) {
  return assertStableCdpBinding(runtime, expectedPid, previous);
}

function assertStableCdpBinding(runtime, expectedPid, previous = null) {
  const currentPid = getVerifiedCdpPid(runtime);
  if (currentPid !== expectedPid) {
    throw new Error(
      `CDP binding PID 漂移（expected=${expectedPid}，current=${currentPid}）。`,
    );
  }
  const cdp = runtime?.cdp?.cdp;
  if (!cdp || typeof cdp.invoke !== "function") {
    throw new Error("当前 CDP binding 已死亡或不可调用；RSS 采样拒绝继续。");
  }
  if (cdp.ws?.readyState !== 1) {
    throw new Error("当前 CDP WebSocket 未保持打开；RSS 采样拒绝继续。");
  }
  const current = {
    binding: runtime.cdp,
    cdp,
    invoke: cdp.invoke,
    ws: cdp.ws,
    pid: currentPid,
    boundPid: runtime.cdp.boundPid,
    socketName: runtime.cdp.socketName,
    socketBindingVerified: runtime.cdp.socketBindingVerified,
  };
  if (
    previous &&
    (current.binding !== previous.binding ||
      current.cdp !== previous.cdp ||
      current.invoke !== previous.invoke ||
      current.ws !== previous.ws ||
      current.pid !== previous.pid ||
      current.boundPid !== previous.boundPid ||
      current.socketName !== previous.socketName ||
      current.socketBindingVerified !== previous.socketBindingVerified)
  ) {
    throw new Error("CDP binding 在采样前后发生漂移；RSS 采样拒绝继续。");
  }
  return current;
}

function readStrictMemory(readMemory, expectedPid) {
  if (!isPid(expectedPid)) {
    throw new Error("严格 RSS 采样缺少有效 expected PID。");
  }
  const reading = readMemory({ requireRss: true, pid: expectedPid });
  if (!reading || typeof reading !== "object") {
    throw new Error("RSS 采样返回空值。");
  }
  if (reading.pid !== expectedPid) {
    throw new Error(
      `RSS 采样 PID 不匹配（expected=${expectedPid}，reading=${reading.pid ?? "missing"}）。`,
    );
  }
  return reading;
}

function assertUsableRss(reading) {
  if (!isRssReading(reading)) {
    throw new Error("RSS 读数缺失或不可用；严格内存门禁拒绝继续。");
  }
}

function unavailableSample(expectedPid, phase, elapsedMs, message) {
  return {
    ...unavailableReading(`RSS 采样失败：${message}`, {
      pid: expectedPid,
      expectedPid,
    }),
    phase,
    elapsedMs,
    error: message,
  };
}

function memorySample(
  runtime,
  readMemory,
  expectedPid,
  phase,
  elapsedMs,
  failures,
) {
  try {
    const binding = assertStableCdpBinding(runtime, expectedPid);
    const reading = readStrictMemory(readMemory, expectedPid);
    assertUsableRss(reading);
    assertStableCdpBinding(runtime, expectedPid, binding);
    return { ...reading, phase, elapsedMs, expectedPid };
  } catch (error) {
    const message = errorText(error);
    failures.push({ phase, message });
    return unavailableSample(expectedPid, phase, elapsedMs, message);
  }
}

function sampleElapsed(now, started) {
  return roundMs(now() - started);
}

function rebuildOutput(
  result,
  invokeError,
  memorySamples,
  failures,
  wallClockMs,
  expectedPid,
) {
  const output = {
    wallClockMs,
    reportedDurationMs: Number.isFinite(result?.durationMs)
      ? result.durationMs
      : null,
    indexedCount: Number.isSafeInteger(result?.indexedCount)
      ? result.indexedCount
      : null,
    expectedPid,
    memorySamples,
    memoryPeak: findMemoryPeak(memorySamples),
    memorySampleCount: memorySamples.length,
    memorySamplingFailures: failures,
    ok: !invokeError,
  };
  if (invokeError) output.error = errorText(invokeError);
  return output;
}

async function rebuildIndex(runtime, dependencies = {}) {
  const readMemory = dependencies.readMemory || readAndroidMemory;
  const timer = dependencies.timer || {};
  const setIntervalFn =
    dependencies.setInterval || timer.setInterval || globalThis.setInterval;
  const clearIntervalFn =
    dependencies.clearInterval || timer.clearInterval || globalThis.clearInterval;
  const intervalMs =
    dependencies.memorySampleIntervalMs ?? MEMORY_SAMPLE_INTERVAL_MS;
  const now = dependencies.now || (() => performance.now());
  const expectedPid = getVerifiedCdpPid(runtime);
  const wallStarted = performance.now();
  const started = now();
  const memorySamples = [];
  const memorySamplingFailures = [];
  const sample = (phase) => {
    memorySamples.push(
      memorySample(
        runtime,
        readMemory,
        expectedPid,
        phase,
        sampleElapsed(now, started),
        memorySamplingFailures,
      ),
    );
  };
  sample("before");
  let intervalHandle = null;
  try {
    intervalHandle = setIntervalFn(() => sample("during"), intervalMs);
  } catch (error) {
    memorySamplingFailures.push({ phase: "timer", message: errorText(error) });
  }
  let result = null;
  let invokeError = null;
  try {
    const invokePromise = runtime.cdp.cdp.invoke(
      "rebuild_messages_fts",
      {},
      { timeoutMs: REBUILD_CDP_TIMEOUT_MS },
    );
    sample("during");
    result = await invokePromise;
  } catch (error) {
    invokeError = error;
  } finally {
    if (intervalHandle !== null) clearIntervalFn(intervalHandle);
    sample("after");
  }
  return rebuildOutput(
    result,
    invokeError,
    memorySamples,
    memorySamplingFailures,
    roundMs(performance.now() - wallStarted),
    expectedPid,
  );
}

module.exports = {
  MEMORY_SAMPLE_INTERVAL_MS,
  assertCdpBinding,
  getVerifiedCdpPid,
  readStrictMemory,
  rebuildIndex,
};
