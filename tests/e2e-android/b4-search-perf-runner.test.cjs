"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  coldSearch,
  openRuntime,
  restartB4ColdRuntime,
  resolveReadMemory,
} = require("./b4-search-perf-runner.cjs");
const {
  COLD_RESTART_STRATEGY,
  COLD_RESTART_STRATEGY_BOUNDARY,
  COLD_RESTART_STRATEGY_REASON,
  buildReport,
} = require("./b4-search-perf-metrics.cjs");
const { readAndroidMemory } = require("./b4-search-perf-memory.cjs");

test("openRuntime 严格按 launch→reconnect→ready，READY 失败时关闭 CDP", async () => {
  const successCalls = [];
  const successCdp = {
    cdp: {},
    close: async () => successCalls.push("close"),
  };
  const runtime = await openRuntime({
    launch: async () => {
      successCalls.push("launch");
      await Promise.resolve();
      successCalls.push("launch-done");
    },
    reconnect: async () => {
      successCalls.push("reconnect");
      return successCdp;
    },
    waitReady: async (candidate) => {
      successCalls.push(["ready", candidate]);
    },
  });
  assert.equal(runtime.cdp, successCdp);
  assert.deepEqual(successCalls, [
    "launch",
    "launch-done",
    "reconnect",
    ["ready", successCdp.cdp],
  ]);

  const failureCalls = [];
  const failure = new Error("核心 READY 等待失败");
  const failureCdp = {
    cdp: {},
    close: async () => failureCalls.push("close"),
  };
  await assert.rejects(
    openRuntime({
      launch: () => failureCalls.push("launch"),
      reconnect: async () => {
        failureCalls.push("reconnect");
        return failureCdp;
      },
      waitReady: async (candidate) => {
        failureCalls.push(["ready", candidate]);
        throw failure;
      },
    }),
    (error) => error === failure,
  );
  assert.deepEqual(failureCalls, [
    "launch",
    "reconnect",
    ["ready", failureCdp.cdp],
    "close",
  ]);
});

test("B4 报告明确记录冷重启策略、原因和边界", () => {
  const report = buildReport(
    { samples: 1, queries: ["needle"], querySource: "cli" },
    { device: null, failures: [] },
  );
  assert.equal(report.configuration.coldRestartStrategy, COLD_RESTART_STRATEGY);
  assert.equal(report.configuration.coldRestartStrategyReason, COLD_RESTART_STRATEGY_REASON);
  assert.equal(report.configuration.coldRestartStrategyBoundary, COLD_RESTART_STRATEGY_BOUNDARY);
  assert.match(report.configuration.coldRestartStrategyReason, /force-stop/);
  assert.match(report.configuration.coldRestartStrategyBoundary, /debug/);
});

test("B4 冷重启使用同 UID direct-kill，并校验旧 PID 消失和新 PID", async () => {
  const calls = [];
  const pids = [null, 7_001];
  const oldRuntime = { pid: 7_000, close: async () => calls.push("close") };
  const nextRuntime = {
    pid: 7_001,
    boundPid: 7_001,
    socketName: "webview_devtools_remote_7001",
    socketBindingVerified: true,
    cdp: {},
  };
  const runtime = { cdp: oldRuntime };
  const result = await restartB4ColdRuntime(runtime, {
    processPid: () => {
      const pid = pids.length > 0 ? pids.shift() : 7_001;
      calls.push(["pid", pid]);
      return pid;
    },
    kill: async (pid) => calls.push(["direct-kill", pid]),
    launch: async () => calls.push("launch"),
    isForeground: () => {
      calls.push("foreground");
      return true;
    },
    reconnect: async () => {
      calls.push("reconnect");
      return nextRuntime;
    },
  });

  assert.equal(runtime.cdp, nextRuntime);
  assert.deepEqual(result, {
    processRestarted: true,
    foregrounded: true,
    previousPid: 7_000,
    nextPid: 7_001,
    cdpPid: 7_001,
  });
  assert.deepEqual(calls, [
    "close",
    ["direct-kill", 7_000],
    ["pid", null],
    "launch",
    ["pid", 7_001],
    "foreground",
    "reconnect",
  ]);
});

test("冷查询每轮都先等 READY，restartMs 包含等待而 durationMs 不包含", async () => {
  let clock = 1_000;
  const calls = [];
  const cdp = {};
  const runtime = { cdp: { cdp } };
  const result = await coldSearch(runtime, ["needle", "body"], 2, {
    now: () => clock,
    restart: async () => {
      calls.push(["restart", calls.filter((call) => call[0] === "restart").length + 1]);
      clock += 25;
      return {
        processRestarted: true,
        foregrounded: true,
        previousPid: 7_000,
        nextPid: 7_001,
        cdpPid: 7_001,
      };
    },
    waitReady: async (candidate) => {
      calls.push(["ready", candidate]);
      clock += 200;
    },
    timedSearch: async (candidate, query, index, phase, restartMs) => {
      calls.push(["search", candidate, query, index, phase, restartMs, clock]);
      clock += 17;
      return {
        index,
        phase,
        query,
        durationMs: 17,
        resultCount: 1,
        nextCursorPresent: false,
        restartMs,
        ok: true,
      };
    },
  });

  assert.equal(result.samples.length, 2);
  assert.deepEqual(
    result.samples.map((sample) => [sample.query, sample.restartMs, sample.durationMs]),
    [["needle", 225, 17], ["body", 225, 17]],
  );
  assert.ok(result.samples.every((sample) => sample.processRestarted));
  assert.ok(result.samples.every((sample) => sample.foregrounded));
  assert.ok(result.samples.every((sample) =>
    sample.previousPid === 7_000 && sample.nextPid === 7_001 && sample.cdpPid === 7_001));
  assert.deepEqual(calls, [
    ["restart", 1],
    ["ready", cdp],
    ["search", cdp, "needle", 1, "cold", 225, 1_225],
    ["restart", 2],
    ["ready", cdp],
    ["search", cdp, "body", 2, "cold", 225, 1_467],
  ]);
});

test("runner 默认内存依赖解析到真实 readAndroidMemory 且可读取 RSS", () => {
  const calls = [];
  const readMemory = resolveReadMemory();
  assert.equal(readMemory, readAndroidMemory);
  const reading = readMemory({
    requireRss: true,
    pid: 4_321,
    runAdb: (args) => {
      calls.push(args);
      if (args.at(-1) === "/proc/4321/status") return "VmRSS: 64 kB\n";
      throw new Error(`不应读取 ${args.join(" ")}`);
    },
  });
  assert.equal(reading.rssAvailable, true);
  assert.equal(reading.pid, 4_321);
  assert.equal(calls.length, 1);
});
