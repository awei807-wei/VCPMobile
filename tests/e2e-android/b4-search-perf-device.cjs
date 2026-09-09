"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const {
  DEBUG_PACKAGE,
  ensureSingleDevice,
  getDeviceInfo,
  runAdb,
  runAdbBuffer,
  runAdbResult,
  runAdbToFile,
} = require("./scripts/adb-env.cjs");
const runtimeSupport = require("./b4-search-perf-device-support.cjs");
const { cleanupError, runCleanupActions } = require("./b4-search-perf-device-operations.cjs");

const TARGET_SERIAL = "emulator-5554";
const TARGET_AVD = "VCPMobile_Perf_16K";
const REMOTE_TEMP = "/data/local/tmp/vcp-b4-fixture.db";
const REMOTE_STAGED_VERIFY = "/data/local/tmp/vcp-b4-fixture-staged.db";
// `run-as <package> ...` starts in /data/user/0/<package>, where the app
// opens vcp_avatar.db. Keep every app-private path as a basename so it stays
// in that same directory; `files/...` would target the wrong database.
const REMOTE_APP_DATA_ROOT = ".";
const appPrivatePath = (basename) => path.posix.join(REMOTE_APP_DATA_ROOT, basename);
const REMOTE_APP_DB = appPrivatePath("vcp_avatar.db");
const REMOTE_APP_DB_WAL = appPrivatePath("vcp_avatar.db-wal");
const REMOTE_APP_DB_SHM = appPrivatePath("vcp_avatar.db-shm");
const REMOTE_STAGING = appPrivatePath(".b4-fixture.staging.db");
const REMOTE_BACKUP = appPrivatePath(".b4-fixture.backup.db");
const REMOTE_BACKUP_WAL = appPrivatePath(".b4-fixture.backup.db-wal");
const REMOTE_BACKUP_SHM = appPrivatePath(".b4-fixture.backup.db-shm");
const REMOTE_DB_SET = Object.freeze([
  [REMOTE_APP_DB, REMOTE_BACKUP],
  [REMOTE_APP_DB_WAL, REMOTE_BACKUP_WAL],
  [REMOTE_APP_DB_SHM, REMOTE_BACKUP_SHM],
]);

function compactError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim().slice(0, 400) || "未知错误";
}

function assertDebugPackage(packageName = DEBUG_PACKAGE) {
  if (packageName !== "com.vcp.avatar.debug") {
    throw new Error(`B4 只允许 debug 包 com.vcp.avatar.debug，收到=${packageName}`);
  }
  return packageName;
}

function assertTargetSerial(serial) {
  if (serial !== undefined && serial !== null && serial !== TARGET_SERIAL) {
    throw new Error(`B4 只允许设备 serial ${TARGET_SERIAL}`);
  }
  if (process.env.ANDROID_SERIAL && process.env.ANDROID_SERIAL !== TARGET_SERIAL) {
    throw new Error(`ANDROID_SERIAL 必须严格为 ${TARGET_SERIAL}`);
  }
}

function assertAvd(runAdbFn = runAdb) {
  const avd = runAdbFn(["shell", "getprop", "ro.boot.qemu.avd_name"]).trim();
  if (avd !== TARGET_AVD) {
    throw new Error(`B4 AVD 必须为 ${TARGET_AVD}，收到=${avd || "empty"}`);
  }
  return avd;
}

function verifyTargetDevice(serial, dependencies = {}) {
  assertTargetSerial(serial);
  const ensureDevice = dependencies.ensureSingleDevice || ensureSingleDevice;
  const runAdbFn = dependencies.runAdb || runAdb;
  process.env.ANDROID_SERIAL = TARGET_SERIAL;
  const device = ensureDevice();
  if (device?.serial !== TARGET_SERIAL) {
    throw new Error("adb 设备 serial 校验失败");
  }
  assertAvd(runAdbFn);
  return {
    serialTargetVerified: true,
    avdVerified: true,
    packageDebug: assertDebugPackage(),
  };
}

function ensureBenchmarkDevice(serial, dependencies = {}) {
  const verified = verifyTargetDevice(serial, dependencies);
  const readInfo = dependencies.getDeviceInfo || getDeviceInfo;
  const info = readInfo();
  if (info?.serial && info.serial !== TARGET_SERIAL) {
    throw new Error("benchmark 设备 serial 校验失败");
  }
  return {
    ...info,
    serialTargetVerified: verified.serialTargetVerified,
    avdVerified: verified.avdVerified,
    packageDebug: "com.vcp.avatar.debug",
  };
}

async function withTargetDevice(serial, action, dependencies = {}) {
  assertTargetSerial(serial);
  const previous = process.env.ANDROID_SERIAL;
  process.env.ANDROID_SERIAL = TARGET_SERIAL;
  try {
    return await action(verifyTargetDevice(serial, dependencies));
  } finally {
    if (previous === undefined) delete process.env.ANDROID_SERIAL;
    else process.env.ANDROID_SERIAL = previous;
  }
}

function parsePid(output) {
  const pid = Number(String(output || "").trim().split(/\s+/)[0]);
  return Number.isSafeInteger(pid) && pid > 0 ? pid : null;
}

function readDebugPid(runAdbFn = runAdb) {
  assertDebugPackage();
  return parsePid(
    runAdbFn(["shell", "pidof", "-s", "com.vcp.avatar.debug"], {
      allowFailure: true,
    }),
  );
}

function sleep(delayMs) {
  return new Promise((resolve) => setTimeout(resolve, delayMs));
}

async function waitDebugPidGone(previousPid, dependencies = {}) {
  const readPid = dependencies.readPid || readDebugPid;
  const wait = dependencies.sleep || sleep;
  const now = dependencies.now || Date.now;
  const timeoutMs = dependencies.timeoutMs || 20_000;
  const pollIntervalMs = dependencies.pollIntervalMs || 100;
  const deadline = now() + timeoutMs;
  while (now() < deadline) {
    if (readPid() !== previousPid) return;
    await wait(Math.min(pollIntervalMs, Math.max(1, deadline - now())));
  }
  throw new Error(`debug 进程 ${previousPid} 未在限定时间内退出`);
}

async function killDebugProcess(dependencies = {}) {
  const packageName = assertDebugPackage(dependencies.packageName);
  const runAdbFn = dependencies.runAdb || runAdb;
  const readPid = dependencies.readPid || (() => readDebugPid(runAdbFn));
  const previousPid = readPid();
  if (!previousPid) return { killed: false, previousPid: null };
  runAdbFn(["shell", "run-as", packageName, "kill", "-9", String(previousPid)]);
  await waitDebugPidGone(previousPid, { ...dependencies, readPid });
  return { killed: true, previousPid };
}

function assertLocalFixture(localPath) {
  if (!localPath || typeof localPath !== "string") throw new Error("fixture path 不能为空");
  const resolved = path.resolve(localPath);
  if (!fs.statSync(resolved, { throwIfNoEntry: false })?.isFile()) {
    throw new Error("fixture 文件不存在");
  }
  return resolved;
}

function localPullPath(prefix = "vcpmobile-b4-device") {
  return path.join(os.tmpdir(), `${prefix}-${process.pid}-${Date.now()}.db`);
}

function stagedTransferError(errorOrResult, bytesWritten = 0) {
  const status = Number.isInteger(errorOrResult?.status) ? errorOrResult.status : 1;
  const reportedBytes = Number.isSafeInteger(errorOrResult?.bytesWritten)
    && errorOrResult.bytesWritten >= 0
    ? errorOrResult.bytesWritten
    : bytesWritten;
  const stderr = String(errorOrResult?.stderr || "")
    .replace(/[^\x20-\x7e\t\r\n]/g, "?")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 400);
  return new Error(
    `staged fixture transfer failed (status=${status}, bytes=${reportedBytes}, stderr=${stderr || "unavailable"})`,
  );
}

function removeLocal(pathname) {
  try {
    if (fs.existsSync(pathname)) fs.unlinkSync(pathname);
  } catch (error) {
    throw new Error(`清理 device 临时文件失败: ${compactError(error)}`);
  }
}

function runRemote(runAdbFn, packageName, command, args = []) {
  return runAdbFn(["shell", "run-as", packageName, command, ...args]);
}

function pullStagedFixture(runAdbFn, options, packageName, stagedLocal) {
  if (typeof options.pullStaged === "function") {
    return options.pullStaged(REMOTE_STAGING, stagedLocal);
  }
  const streamToFile = options.runAdbToFile || (runAdbFn === runAdb ? runAdbToFile : null);
  if (streamToFile) {
    try {
      const result = streamToFile(
        ["exec-out", "run-as", packageName, "cat", REMOTE_STAGING],
        stagedLocal,
      );
      if (result && Number.isInteger(result.status) && result.status !== 0) {
        throw stagedTransferError(result);
      }
      return result;
    } catch (error) {
      if (error?.message?.startsWith("staged fixture transfer failed")) throw error;
      throw stagedTransferError(error);
    }
  }
  const readBuffer = options.runAdbBuffer || (runAdbFn === runAdb ? runAdbBuffer : null);
  if (readBuffer) {
    try {
      const bytes = readBuffer(["exec-out", "run-as", packageName, "cat", REMOTE_STAGING]);
      fs.writeFileSync(stagedLocal, Buffer.isBuffer(bytes) ? bytes : Buffer.from(bytes));
      return;
    } catch (error) {
      throw stagedTransferError(error);
    }
  }
  runRemote(runAdbFn, packageName, "cp", [REMOTE_STAGING, REMOTE_STAGED_VERIFY]);
  runAdbFn(["pull", REMOTE_STAGED_VERIFY, stagedLocal]);
}

function resultRunner(runAdbFn, dependencies = {}) {
  if (dependencies.runAdbResult) return dependencies.runAdbResult;
  if (runAdbFn === runAdb) return runAdbResult;
  return (args, options = {}) => {
    try {
      return { status: 0, stdout: runAdbFn(args, options), stderr: "" };
    } catch (error) {
      if (Number.isInteger(error?.status)) {
        return { status: error.status, stdout: "", stderr: compactError(error) };
      }
      throw error;
    }
  };
}

function remoteFileExists(runAdbFn, dependencies, packageName, remotePath) {
  const result = resultRunner(runAdbFn, dependencies)(
    ["shell", "run-as", packageName, "test", "-e", remotePath],
    { allowFailure: true },
  );
  if (result.status === 0) return true;
  if (result.status === 1) return false;
  throw new Error(`检查 app-private fixture 路径失败: ${compactError(result.stderr)}`);
}

async function rollbackRemoteFixture(runAdbFn, dependencies, packageName, moved, commitAttempted) {
  const failures = [];
  if (commitAttempted) {
    failures.push(...await runCleanupActions([
      ["rollback-current-db", () => removeRemoteFile(runAdbFn, packageName, REMOTE_APP_DB)],
      ["rollback-staging", () => removeRemoteFile(runAdbFn, packageName, REMOTE_STAGING)],
    ]));
  }
  for (const [source, backup] of [...moved].reverse()) {
    try {
      const backupExists = remoteFileExists(runAdbFn, dependencies, packageName, backup);
      const sourceExists = remoteFileExists(runAdbFn, dependencies, packageName, source);
      if (backupExists && !sourceExists) runRemote(runAdbFn, packageName, "mv", [backup, source]);
    } catch (error) {
      const residualPaths = [];
      for (const candidate of [source, backup]) {
        try {
          if (remoteFileExists(runAdbFn, dependencies, packageName, candidate)) {
            residualPaths.push(candidate);
          }
        } catch {
          residualPaths.push(candidate);
        }
      }
      failures.push({
        phase: `rollback-${source}`,
        message: compactError(error),
        ...(residualPaths.length ? { residualPaths } : {}),
      });
    }
  }
  return failures;
}

async function stageFixture(runAdbFn, options, packageName, source, stagedLocal) {
  runAdbFn(["push", source, REMOTE_TEMP]);
  runRemote(runAdbFn, packageName, "cp", [REMOTE_TEMP, REMOTE_STAGING]);
  pullStagedFixture(runAdbFn, options, packageName, stagedLocal);
  const verification = await options.verifyLocal(stagedLocal);
  if (!verification?.ok) throw new Error("staged fixture 未通过本地 verify");
}

function commitStagedFixture(runAdbFn, options, packageName, state) {
  runRemote(runAdbFn, packageName, "rm", [
    "-f",
    REMOTE_BACKUP,
    REMOTE_BACKUP_WAL,
    REMOTE_BACKUP_SHM,
  ]);
  for (const [sourcePath, backupPath] of REMOTE_DB_SET) {
    if (remoteFileExists(runAdbFn, options, packageName, sourcePath)) {
      runRemote(runAdbFn, packageName, "mv", [sourcePath, backupPath]);
      state.moved.push([sourcePath, backupPath]);
    }
  }
  state.commitAttempted = true;
  runRemote(runAdbFn, packageName, "mv", [REMOTE_STAGING, REMOTE_APP_DB]);
  state.committed = true;
  runRemote(runAdbFn, packageName, "rm", [
    "-f",
    REMOTE_BACKUP,
    REMOTE_BACKUP_WAL,
    REMOTE_BACKUP_SHM,
  ]);
}

function removeAdbPath(runAdbFn, args, remotePath) {
  try {
    runAdbFn(args);
  } catch (error) {
    error.residualPaths = [remotePath];
    throw error;
  }
}

function removeLocalPath(pathname) {
  try {
    removeLocal(pathname);
  } catch (error) {
    error.residualPaths = [pathname];
    throw error;
  }
}

async function removeRemoteFile(runAdbFn, packageName, remotePath) {
  try {
    runRemote(runAdbFn, packageName, "rm", ["-f", remotePath]);
  } catch (error) {
    error.residualPaths = [remotePath];
    throw error;
  }
}

function removeRemoteFiles(runAdbFn, packageName, remotePaths) {
  try {
    runRemote(runAdbFn, packageName, "rm", ["-f", ...remotePaths]);
  } catch (error) {
    error.residualPaths = remotePaths;
    throw error;
  }
}

async function injectFixture(localPath, options = {}) {
  const source = assertLocalFixture(localPath);
  const packageName = assertDebugPackage(options.packageName);
  if (typeof options.verifyLocal !== "function") {
    throw new Error("注入前必须提供 staged fixture 校验器");
  }
  const runAdbFn = options.runAdb || runAdb;
  return withTargetDevice(options.serial, async (device) => {
    const stagedLocal = localPullPath("vcpmobile-b4-staged");
    const state = { moved: [], commitAttempted: false, committed: false };
    let result = null;
    let primary = null;
    try {
      await killDebugProcess({ ...options, runAdb: runAdbFn });
      await stageFixture(runAdbFn, options, packageName, source, stagedLocal);
      commitStagedFixture(runAdbFn, options, packageName, state);
      result = {
        ...device,
        injected: true,
        stagedFixtureVerified: true,
        packageDebug: packageName,
      };
    } catch (error) {
      primary = error;
      // A failure while moving the old DB/WAL/SHM can happen before
      // commitAttempted is set. Moved entries still need restoring.
      if (!state.committed && (state.commitAttempted || state.moved.length > 0)) {
        const rollbackFailures = await rollbackRemoteFixture(
          runAdbFn,
          options,
          packageName,
          state.moved,
          true,
        );
        if (rollbackFailures.length) {
          primary = cleanupError(primary, rollbackFailures);
        }
      }
    }
    const cleanupFailures = await runCleanupActions([
      ["remote-temp", () => removeAdbPath(runAdbFn, ["shell", "rm", "-f", REMOTE_TEMP], REMOTE_TEMP)],
      ["remote-staged-verify", () => removeAdbPath(runAdbFn, ["shell", "rm", "-f", REMOTE_STAGED_VERIFY], REMOTE_STAGED_VERIFY)],
      ["local-staged-verify", () => removeLocalPath(stagedLocal)],
      ["remote-staging", () => removeRemoteFile(runAdbFn, packageName, REMOTE_STAGING)],
    ]);
    if (primary || cleanupFailures.length) throw cleanupError(primary, cleanupFailures);
    return result;
  }, options);
}

async function verifyDeviceFixture(options = {}) {
  const packageName = assertDebugPackage(options.packageName);
  const runAdbFn = options.runAdb || runAdb;
  return withTargetDevice(options.serial, async (device) => {
    const readPid = options.readPid || (() => readDebugPid(runAdbFn));
    const pid = readPid();
    if (!pid) throw new Error("debug 应用未运行，无法读取运行态 fixture");
    const runtimeResult = await (options.readRuntimeStatus || runtimeSupport.readRustRuntimeStatus)(options);
    const semanticChecks = runtimeSupport.normalizeSemanticChecks(runtimeResult?.semanticChecks);
    const verification = runtimeSupport.runtimeFixtureChecks(runtimeResult?.status || runtimeResult, semanticChecks);
    if (!verification.valid) throw new Error("运行态 B4 fixture 校验失败");
    return {
      ...device,
      ...verification.dto,
      ok: true,
      semanticChecks,
      semanticValid: true,
      runtimeStatusVerified: true,
      deviceFixtureVerified: true,
      command: "verify-device",
      packageDebug: packageName,
    };
  }, options);
}

async function cleanupDebugFixture(options = {}) {
  const packageName = assertDebugPackage(options.packageName);
  const runAdbFn = options.runAdb || runAdb;
  return withTargetDevice(options.serial, async (device) => {
    const failures = [];
    try {
      await killDebugProcess({ ...options, runAdb: runAdbFn });
    } catch (error) {
      failures.push({ phase: "restart", message: compactError(error) });
    }
    failures.push(...await runCleanupActions([
      ["remote-database", () => removeRemoteFiles(runAdbFn, packageName, [REMOTE_APP_DB, REMOTE_APP_DB_WAL, REMOTE_APP_DB_SHM])],
      ["remote-temp", () => removeAdbPath(runAdbFn, ["shell", "rm", "-f", REMOTE_TEMP], REMOTE_TEMP)],
      ["remote-staging", () => removeRemoteFiles(runAdbFn, packageName, [REMOTE_STAGING, REMOTE_BACKUP, REMOTE_BACKUP_WAL, REMOTE_BACKUP_SHM])],
    ]));
    if (failures.length) throw cleanupError(null, failures);
    return { ...device, command: "cleanup-device", ok: true, cleaned: true, packageDebug: packageName };
  }, options);
}

module.exports = {
  INDEX_STATUS_TIMEOUT_MS: runtimeSupport.INDEX_STATUS_TIMEOUT_MS,
  REMOTE_APP_DATA_ROOT,
  REMOTE_APP_DB,
  REMOTE_APP_DB_SHM,
  REMOTE_APP_DB_WAL,
  REMOTE_BACKUP,
  REMOTE_BACKUP_SHM,
  REMOTE_BACKUP_WAL,
  REMOTE_STAGED_VERIFY,
  REMOTE_STAGING,
  REMOTE_TEMP,
  TARGET_AVD,
  TARGET_SERIAL,
  assertDebugPackage,
  assertTargetSerial,
  cleanupDebugFixture,
  ensureBenchmarkDevice,
  injectFixture,
  killDebugProcess,
  normalizeRuntimeStatus: runtimeSupport.normalizeRuntimeStatus,
  pullStagedFixture,
  readDebugPid,
  readRustRuntimeStatus: runtimeSupport.readRustRuntimeStatus,
  runtimeFixtureChecks: runtimeSupport.runtimeFixtureChecks,
  verifyDeviceFixture,
  verifyTargetDevice,
  waitDebugPidGone,
  withTargetDevice,
};
