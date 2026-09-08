"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  coldSearch,
} = require("./b4-search-perf-runner.cjs");
const {
  buildDirectKillArgs,
  restartB4ColdRuntime,
} = require("./b4-search-perf-restart.cjs");

const OLD_PID = 7_000;
const NEW_PID = 7_001;

function clockDependencies(timeoutMs = 250) {
  let clock = 0;
  return {
    now: () => clock,
    sleep: async (delayMs) => {
      clock += delayMs;
    },
    timeoutMs,
    pollIntervalMs: 100,
  };
}

function oldRuntime(close = async () => {}) {
  return { cdp: { pid: OLD_PID, close } };
}

test("默认 direct-kill argv 完整包含 shell/run-as/debug 包/PID", () => {
  assert.deepEqual(buildDirectKillArgs(OLD_PID), [
    "shell",
    "run-as",
    "com.vcp.avatar.debug",
    "kill",
    "-9",
    String(OLD_PID),
  ]);
});

test("默认 direct-kill 使用 fail-fast，并透传 adb 失败", async () => {
  const failure = new Error("adb kill failed");
  let closeCount = 0;
  await assert.rejects(
    restartB4ColdRuntime(oldRuntime(async () => {
      closeCount += 1;
    }), {
      runAdb: (args, options) => {
        assert.deepEqual(args, buildDirectKillArgs(OLD_PID));
        assert.equal(options.allowFailure, false);
        throw failure;
      },
    }),
    (error) => error === failure,
  );
  assert.equal(closeCount, 1);
});

test("旧 PID 未消失时立即以旧进程退出超时失败", async () => {
  const dependencies = clockDependencies();
  let launchCount = 0;
  await assert.rejects(
    restartB4ColdRuntime(oldRuntime(), {
      ...dependencies,
      processPid: () => OLD_PID,
      kill: async () => {},
      launch: async () => {
        launchCount += 1;
      },
    }),
    (error) => error.code === "ANDROID_OLD_PROCESS_EXIT_TIMEOUT",
  );
  assert.equal(launchCount, 0);
});

test("新 PID 始终等于旧 PID 时以新进程超时失败", async () => {
  const dependencies = clockDependencies();
  let processReads = 0;
  let foregroundCount = 0;
  await assert.rejects(
    restartB4ColdRuntime(oldRuntime(), {
      ...dependencies,
      processPid: () => {
        processReads += 1;
        return processReads === 1 ? null : OLD_PID;
      },
      kill: async () => {},
      launch: async () => {},
      isForeground: () => {
        foregroundCount += 1;
        return true;
      },
    }),
    (error) => error.code === "ANDROID_NEW_PROCESS_TIMEOUT",
  );
  assert.equal(foregroundCount, 0);
});

test("新 PID 出现但前台恢复超时时失败", async () => {
  const dependencies = clockDependencies();
  let processReads = 0;
  let reconnectCount = 0;
  await assert.rejects(
    restartB4ColdRuntime(oldRuntime(), {
      ...dependencies,
      processPid: () => {
        processReads += 1;
        return processReads === 1 ? null : NEW_PID;
      },
      kill: async () => {},
      launch: async () => {},
      isForeground: () => false,
      reconnect: async () => {
        reconnectCount += 1;
        return { pid: NEW_PID, close: async () => {} };
      },
    }),
    (error) => error.code === "ANDROID_FOREGROUND_TIMEOUT",
  );
  assert.equal(reconnectCount, 0);
});

test("CDP PID mismatch 会关闭候选并阻止 READY/search，runtime.cdp 保持 null", async () => {
  const runtime = oldRuntime();
  const candidate = {
    pid: NEW_PID + 100,
    close: async () => {
      candidate.closed = true;
    },
  };
  const dependencies = clockDependencies();
  let processReads = 0;
  let readyCount = 0;
  let searchCount = 0;
  const result = await coldSearch(runtime, ["needle"], 1, {
    ...dependencies,
    restart: (candidateRuntime) =>
      restartB4ColdRuntime(candidateRuntime, {
        ...dependencies,
        processPid: () => {
          processReads += 1;
          return processReads === 1 ? null : NEW_PID;
        },
        kill: async () => {},
        launch: async () => {},
        isForeground: () => true,
        reconnect: async () => candidate,
      }),
    waitReady: async () => {
      readyCount += 1;
    },
    timedSearch: async () => {
      searchCount += 1;
      return { ok: true, durationMs: 1 };
    },
  });

  assert.equal(runtime.cdp, null);
  assert.equal(candidate.closed, true);
  assert.equal(readyCount, 0);
  assert.equal(searchCount, 0);
  assert.equal(result.samples.length, 1);
  assert.equal(result.samples[0].ok, false);
  assert.equal(
    result.samples[0].errorCode,
    "ANDROID_NEW_PROCESS_PID_MISMATCH",
  );
  assert.match(result.samples[0].error, /CDP 重连未证明绑定新进程/);
});
