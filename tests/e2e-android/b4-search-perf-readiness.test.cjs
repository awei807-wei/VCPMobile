"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  CORE_READY_TIMEOUT_MS,
  normalizeCoreStatus,
  waitForCoreReady,
} = require("./b4-search-perf-runner.cjs");

test("核心状态从 initializing/optimizing 到 ready 时按序放行", async () => {
  let clock = 0;
  const statuses = ["initializing", "optimizing", "ready"];
  const calls = [];
  const sleeps = [];
  const result = await waitForCoreReady(
    {
      invoke: async (command, args, options) => {
        calls.push({ command, args, options });
        return statuses.shift();
      },
    },
    {
      timeoutMs: 1_000,
      pollIntervalMs: 100,
      now: () => clock,
      sleep: async (delayMs) => {
        sleeps.push(delayMs);
        clock += delayMs;
      },
    },
  );

  assert.equal(result.status, "ready");
  assert.equal(result.elapsedMs, 200);
  assert.deepEqual(
    calls.map((call) => call.command),
    ["get_core_status", "get_core_status", "get_core_status"],
  );
  assert.deepEqual(sleeps, [100, 100]);
});

test("核心状态归一化覆盖字符串和嵌套对象形态", () => {
  const cases = [
    ["initializing", "initializing"],
    ["optimizing", "optimizing"],
    ["READY", "ready"],
    ["error", "error"],
    [{ status: "ready" }, "ready"],
    [{ state: "optimizing" }, "optimizing"],
    [{ core: { status: "initializing" } }, "initializing"],
    [{ core: { state: "error" } }, "error"],
    [null, null],
  ];
  for (const [value, expected] of cases) {
    assert.equal(normalizeCoreStatus(value), expected);
  }
});

test("核心 error 状态读取 get_last_error 后立即失败", async () => {
  const calls = [];
  await assert.rejects(
    waitForCoreReady(
      {
        invoke: async (command) => {
          calls.push(command);
          if (command === "get_core_status") return "error";
          return "数据库初始化失败：测试错误";
        },
      },
      { timeoutMs: 1_000, sleep: async () => {} },
    ),
    (error) => {
      assert.equal(error.code, "CORE_STARTUP_FAILED");
      assert.match(error.message, /数据库初始化失败/);
      return true;
    },
  );
  assert.deepEqual(calls, ["get_core_status", "get_last_error"]);
});

test("get_last_error 读取失败仍以 CORE_STARTUP_FAILED 结束", async () => {
  const calls = [];
  await assert.rejects(
    waitForCoreReady(
      {
        invoke: async (command) => {
          calls.push(command);
          if (command === "get_core_status") return "error";
          throw new Error("诊断命令暂不可用");
        },
      },
      { timeoutMs: 1_000, sleep: async () => {} },
    ),
    (error) => {
      assert.equal(error.code, "CORE_STARTUP_FAILED");
      assert.match(error.message, /读取 get_last_error 失败/);
      return true;
    },
  );
  assert.deepEqual(calls, ["get_core_status", "get_last_error"]);
});

test("核心始终 initializing 时在独立预算内超时", async () => {
  let clock = 0;
  const calls = [];
  const sleeps = [];
  await assert.rejects(
    waitForCoreReady(
      {
        invoke: async (command, args, options) => {
          calls.push({ command, options });
          return "initializing";
        },
      },
      {
        timeoutMs: 450,
        pollIntervalMs: 100,
        now: () => clock,
        sleep: async (delayMs) => {
          sleeps.push(delayMs);
          clock += delayMs;
        },
      },
    ),
    (error) => {
      assert.equal(error.code, "CORE_READY_TIMEOUT");
      assert.match(error.message, /预算 450ms/);
      assert.equal(error.lastStatus, "initializing");
      return true;
    },
  );
  assert.deepEqual(
    calls.map((call) => call.command),
    [
      "get_core_status",
      "get_core_status",
      "get_core_status",
      "get_core_status",
      "get_core_status",
    ],
  );
  assert.deepEqual(
    calls.map((call) => call.options.timeoutMs),
    [450, 350, 250, 150, 50],
  );
  assert.deepEqual(sleeps, [100, 100, 100, 100, 50]);
});

test("核心返回非预期状态时立即失败", async () => {
  let calls = 0;
  await assert.rejects(
    waitForCoreReady(
      {
        invoke: async () => {
          calls += 1;
          return "paused";
        },
      },
      { timeoutMs: CORE_READY_TIMEOUT_MS, sleep: async () => {} },
    ),
    (error) => {
      assert.equal(error.code, "CORE_STATUS_UNEXPECTED");
      assert.match(error.message, /paused/);
      return true;
    },
  );
  assert.equal(calls, 1);
});

test("核心状态调用错误不重试", async () => {
  let calls = 0;
  const expected = new Error("CDP 调用失败：连接断开");
  await assert.rejects(
    waitForCoreReady(
      {
        invoke: async () => {
          calls += 1;
          throw expected;
        },
      },
      { timeoutMs: CORE_READY_TIMEOUT_MS, sleep: async () => {} },
    ),
    (error) => {
      assert.equal(error, expected);
      return true;
    },
  );
  assert.equal(calls, 1);
});
