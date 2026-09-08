"use strict";

const { DEBUG_PACKAGE, runAdb } = require("./scripts/adb-env.cjs");
const { isForeground, reconnectAndroidCdp } = require("./wire14/android-state.cjs");

const COLD_RESTART_TIMEOUT_MS = 20_000;
const COLD_RESTART_POLL_INTERVAL_MS = 100;

function buildDirectKillArgs(pid, packageName = DEBUG_PACKAGE) {
  return ["shell", "run-as", packageName, "kill", "-9", String(pid)];
}

function codedError(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}

function hasVerifiedPidSocket(runtime, pid) {
  return (
    runtime?.socketBindingVerified === true &&
    runtime?.boundPid === pid &&
    runtime?.socketName === `webview_devtools_remote_${pid}`
  );
}

function readProcessPid() {
  const output = runAdb(["shell", "pidof", "-s", DEBUG_PACKAGE], {
    allowFailure: true,
  }).trim();
  const pid = Number(output.split(/\s+/)[0]);
  return Number.isSafeInteger(pid) && pid > 0 ? pid : null;
}

function defaultSleep(delayMs) {
  return new Promise((resolve) => setTimeout(resolve, delayMs));
}

async function waitForPidGone(previousPid, dependencies = {}) {
  const processPid = dependencies.processPid || readProcessPid;
  const now = dependencies.now || Date.now;
  const sleep = dependencies.sleep || defaultSleep;
  const timeoutMs = dependencies.timeoutMs || COLD_RESTART_TIMEOUT_MS;
  const pollIntervalMs =
    dependencies.pollIntervalMs || COLD_RESTART_POLL_INTERVAL_MS;
  const deadline = now() + timeoutMs;
  while (now() < deadline) {
    if (processPid() !== previousPid) return;
    const remainingMs = deadline - now();
    await sleep(Math.min(pollIntervalMs, Math.max(1, remainingMs)));
  }
  throw codedError(
    "ANDROID_OLD_PROCESS_EXIT_TIMEOUT",
    `旧 Android 进程 ${previousPid} 未在限定时间内退出`,
  );
}

async function waitForNewProcess(previousPid, dependencies = {}) {
  const processPid = dependencies.processPid || readProcessPid;
  const now = dependencies.now || Date.now;
  const sleep = dependencies.sleep || defaultSleep;
  const timeoutMs = dependencies.timeoutMs || COLD_RESTART_TIMEOUT_MS;
  const pollIntervalMs =
    dependencies.pollIntervalMs || COLD_RESTART_POLL_INTERVAL_MS;
  const deadline = now() + timeoutMs;
  while (now() < deadline) {
    const nextPid = processPid();
    if (nextPid && nextPid !== previousPid) return nextPid;
    const remainingMs = deadline - now();
    await sleep(Math.min(pollIntervalMs, Math.max(1, remainingMs)));
  }
  throw codedError(
    "ANDROID_NEW_PROCESS_TIMEOUT",
    `Android Debug 应用未在限定时间内生成新进程（旧 PID=${previousPid}）`,
  );
}

async function waitForForegroundReady(dependencies = {}) {
  const checkForeground = dependencies.isForeground || isForeground;
  const now = dependencies.now || Date.now;
  const sleep = dependencies.sleep || defaultSleep;
  const timeoutMs = dependencies.timeoutMs || COLD_RESTART_TIMEOUT_MS;
  const pollIntervalMs =
    dependencies.pollIntervalMs || COLD_RESTART_POLL_INTERVAL_MS;
  const deadline = now() + timeoutMs;
  while (now() < deadline) {
    if (checkForeground()) return true;
    const remainingMs = deadline - now();
    await sleep(Math.min(pollIntervalMs, Math.max(1, remainingMs)));
  }
  throw codedError(
    "ANDROID_FOREGROUND_TIMEOUT",
    "Android Debug 应用重启后未回到前台",
  );
}

/** Restart only the B4 debug app process without force-stopping its task. */
async function restartB4ColdRuntime(runtime, dependencies = {}) {
  const previousRuntime = runtime?.cdp;
  const processPid = dependencies.processPid || readProcessPid;
  const previousPid = previousRuntime?.pid || processPid();
  if (!Number.isSafeInteger(previousPid) || previousPid <= 0) {
    throw codedError("ANDROID_OLD_PROCESS_PID_UNAVAILABLE", "无法确定旧 Android 进程 PID");
  }

  runtime.cdp = null;
  if (previousRuntime) await previousRuntime.close().catch(() => {});

  const kill =
    dependencies.kill ||
    ((pid) =>
      (dependencies.runAdb || runAdb)(buildDirectKillArgs(pid), {
        allowFailure: false,
      }));
  await kill(previousPid);

  const waitGone = dependencies.waitPidGone || waitForPidGone;
  await waitGone(previousPid, { ...dependencies, processPid });

  const launch =
    dependencies.launch ||
    (() =>
      runAdb(["shell", "monkey", "-p", DEBUG_PACKAGE, "1"], {
        allowFailure: false,
      }));
  await launch();

  const waitNew = dependencies.waitNewProcess || waitForNewProcess;
  const nextPid = await waitNew(previousPid, { ...dependencies, processPid });
  const foregrounded = await waitForForegroundReady({ ...dependencies });
  const reconnect = dependencies.reconnect || reconnectAndroidCdp;
  const nextRuntime = await reconnect();
  if (
    !Number.isSafeInteger(nextRuntime?.pid) ||
    nextRuntime.pid !== nextPid ||
    !hasVerifiedPidSocket(nextRuntime, nextPid)
  ) {
    if (typeof nextRuntime?.close === "function") {
      await nextRuntime.close().catch(() => {});
    }
    throw codedError(
      "ANDROID_NEW_PROCESS_PID_MISMATCH",
      `CDP 重连未证明绑定新进程 PID-specific socket（等待=${nextPid}，CDP=${nextRuntime?.pid ?? "unknown"}，绑定=${nextRuntime?.boundPid ?? "unknown"}）`,
    );
  }
  runtime.cdp = nextRuntime;
  return {
    processRestarted: nextPid !== previousPid,
    foregrounded,
    previousPid,
    nextPid,
    cdpPid: nextRuntime.pid,
  };
}

module.exports = {
  buildDirectKillArgs,
  COLD_RESTART_POLL_INTERVAL_MS,
  COLD_RESTART_TIMEOUT_MS,
  readProcessPid,
  restartB4ColdRuntime,
  waitForForegroundReady,
  waitForNewProcess,
  waitForPidGone,
};
