"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const device = require("./b4-search-perf-device.cjs");

function runtimeStatus() {
  return {
    available: true,
    schemaValid: true,
    tokenizerValid: true,
    healthy: true,
    topicCount: 1_800,
    liveTopicRowCount: 1_800,
    liveCount: 50_000,
    indexedCount: 50_000,
    missingCount: 0,
    orphanCount: 0,
    duplicateCount: 0,
    staleCount: 0,
    decodeErrorCount: 0,
    decodedContentBytes: 100 * 1024 * 1024,
  };
}

function semanticChecks() {
  return [
    ["default-zh-1", true],
    ["default-zh-2", true],
    ["default-zh-3", true],
    ["default-en", true],
    ["default-special", true],
    ["default-empty", false],
  ].map(([caseId, nonEmpty]) => ({ caseId, nonEmpty, passed: true }));
}

function harness({ failCommit = false, failBackupMove = false } = {}) {
  const calls = [];
  const remote = new Set([
    device.REMOTE_APP_DB,
    device.REMOTE_APP_DB_WAL,
    device.REMOTE_APP_DB_SHM,
  ]);
  const runAdb = (args) => {
    calls.push(["adb", args]);
    if (args[0] === "shell" && args[1] === "getprop") return device.TARGET_AVD;
    if (args[0] === "push") {
      remote.add(device.REMOTE_TEMP);
      return "";
    }
    if (args[0] === "shell" && args[1] === "rm") {
      for (const item of args.slice(3)) remote.delete(item);
      return "";
    }
    if (args[0] !== "shell" || args[1] !== "run-as") return "";
    const command = args[3];
    const commandArgs = args.slice(4);
    if (command === "cp") remote.add(commandArgs[1]);
    if (command === "rm") {
      for (const item of commandArgs.slice(1)) remote.delete(item);
    }
    if (command === "mv") {
      const [source, destination] = commandArgs;
      if (failBackupMove && source === device.REMOTE_APP_DB_WAL && destination === device.REMOTE_BACKUP_WAL) {
        throw new Error("injected backup rename failure");
      }
      if (failCommit && source === device.REMOTE_STAGING && destination === device.REMOTE_APP_DB) {
        throw new Error("injected atomic rename failure");
      }
      remote.delete(source);
      remote.add(destination);
    }
    return "";
  };
  const runAdbResult = (args) => {
    calls.push(["result", args]);
    const remotePath = args.at(-1);
    return { status: remote.has(remotePath) ? 0 : 1, stdout: "", stderr: "" };
  };
  return {
    calls,
    remote,
    runAdb,
    runAdbResult,
    ensureSingleDevice: () => ({ serial: device.TARGET_SERIAL }),
    readPid: () => null,
    pullStaged: (_remote, local) => fs.writeFileSync(local, "staged"),
  };
}

function fixtureFile() {
  const pathname = path.join(
    fs.mkdtempSync(path.join(os.tmpdir(), "b4-device-test-")),
    "fixture.db",
  );
  fs.writeFileSync(pathname, "fixture");
  return pathname;
}

function cleanupFixtureFile(pathname) {
  try {
    fs.unlinkSync(pathname);
  } catch {}
  try {
    fs.rmdirSync(path.dirname(pathname));
  } catch {}
}

test("app-private fixture 路径都位于 run-as data root，且不再误指向 files/", () => {
  const appPrivatePaths = [
    device.REMOTE_APP_DB,
    device.REMOTE_APP_DB_WAL,
    device.REMOTE_APP_DB_SHM,
    device.REMOTE_STAGING,
    device.REMOTE_BACKUP,
    device.REMOTE_BACKUP_WAL,
    device.REMOTE_BACKUP_SHM,
  ];
  assert.equal(device.REMOTE_APP_DATA_ROOT, ".");
  assert.deepEqual(appPrivatePaths, [
    "vcp_avatar.db",
    "vcp_avatar.db-wal",
    "vcp_avatar.db-shm",
    ".b4-fixture.staging.db",
    ".b4-fixture.backup.db",
    ".b4-fixture.backup.db-wal",
    ".b4-fixture.backup.db-shm",
  ]);
  for (const remotePath of appPrivatePaths) {
    assert.equal(path.posix.dirname(remotePath), ".", remotePath);
    assert.equal(remotePath.startsWith("files/"), false, remotePath);
  }
});

test("注入只使用 app-private staging 和同文件系统 rename，成功后不伪造 remoteDatabaseVerified", async () => {
  const pathname = fixtureFile();
  const fake = harness();
  try {
    const result = await device.injectFixture(pathname, {
      runAdb: fake.runAdb,
      runAdbResult: fake.runAdbResult,
      ensureSingleDevice: fake.ensureSingleDevice,
      readPid: fake.readPid,
      pullStaged: fake.pullStaged,
      verifyLocal: async () => ({ ok: true }),
      sleep: async () => {},
    });
    assert.equal(result.stagedFixtureVerified, true);
    assert.equal(result.remoteDatabaseVerified, undefined);
    assert.equal(fake.remote.has(device.REMOTE_APP_DB), true);
    assert.equal(fake.remote.has(device.REMOTE_APP_DB_WAL), false);
    assert.ok(fake.calls.some(([, args]) => args.includes(device.REMOTE_STAGING)));
    assert.ok(fake.calls.some(([, args]) => args.includes("mv")));
    assert.ok(fake.calls.every(([, args]) => !args.includes("am") && !args.includes("force-stop")));
  } finally {
    cleanupFixtureFile(pathname);
  }
});

test("大于 32MiB 的 staged fixture 通过 runAdbToFile 依赖注入直接写入本地文件", async () => {
  const pathname = fixtureFile();
  const fake = harness();
  const calls = [];
  const transferBytes = 33 * 1024 * 1024;
  try {
    const result = await device.injectFixture(pathname, {
      runAdb: fake.runAdb,
      runAdbResult: fake.runAdbResult,
      runAdbToFile: (args, destination) => {
        calls.push({ args, destination });
        const fd = fs.openSync(destination, "w");
        try {
          fs.ftruncateSync(fd, transferBytes);
        } finally {
          fs.closeSync(fd);
        }
        return { status: 0, bytesWritten: transferBytes, stderr: "" };
      },
      runAdbBuffer: () => {
        throw new Error("runAdbBuffer must not be used for staged fixture");
      },
      ensureSingleDevice: fake.ensureSingleDevice,
      readPid: fake.readPid,
      verifyLocal: async (localPath) => ({ ok: fs.statSync(localPath).size === transferBytes }),
    });
    assert.equal(result.stagedFixtureVerified, true);
    assert.equal(calls.length, 1);
    assert.deepEqual(calls[0].args, [
      "exec-out",
      "run-as",
      "com.vcp.avatar.debug",
      "cat",
      device.REMOTE_STAGING,
    ]);
    assert.equal(fake.remote.has(device.REMOTE_STAGING), false);
  } finally {
    cleanupFixtureFile(pathname);
  }
});

test("stage 传输失败不泄露二进制 stdout，并清理 app-private staging；清理失败报告真实 residual", async () => {
  const pathname = fixtureFile();
  const fake = harness();
  const originalRunAdb = fake.runAdb;
  const secret = "SQLITE_BINARY_SECRET";
  fake.runAdb = (args) => {
    if (args[0] === "shell" && args[1] === "run-as" && args[3] === "rm" && args.includes(device.REMOTE_STAGING)) {
      throw new Error("staging cleanup unavailable");
    }
    return originalRunAdb(args);
  };
  try {
    await assert.rejects(
      device.injectFixture(pathname, {
        runAdb: fake.runAdb,
        runAdbResult: fake.runAdbResult,
        runAdbToFile: () => ({
          status: 7,
          bytesWritten: secret.length,
          stderr: "transfer failed",
          stdout: secret,
        }),
        ensureSingleDevice: fake.ensureSingleDevice,
        readPid: fake.readPid,
        verifyLocal: async () => ({ ok: true }),
      }),
      (error) => {
        assert.equal(error.message.includes(secret), false);
        const stagingFailure = error.cleanupFailures.find((failure) => failure.phase === "remote-staging");
        assert.deepEqual(stagingFailure.residualPaths, [device.REMOTE_STAGING]);
        return true;
      },
    );
    assert.equal(fake.remote.has(device.REMOTE_STAGING), true);
    assert.equal(fake.remote.has(device.REMOTE_APP_DB), true);
  } finally {
    cleanupFixtureFile(pathname);
  }
});

test("原子 rename 失败恢复 DB/WAL/SHM 三件套并清理临时文件", async () => {
  const pathname = fixtureFile();
  const fake = harness({ failCommit: true });
  try {
    await assert.rejects(
      device.injectFixture(pathname, {
        runAdb: fake.runAdb,
        runAdbResult: fake.runAdbResult,
        ensureSingleDevice: fake.ensureSingleDevice,
        readPid: fake.readPid,
        pullStaged: fake.pullStaged,
        verifyLocal: async () => ({ ok: true }),
      }),
      /atomic rename failure/,
    );
    for (const remotePath of [device.REMOTE_APP_DB, device.REMOTE_APP_DB_WAL, device.REMOTE_APP_DB_SHM]) {
      assert.equal(fake.remote.has(remotePath), true, remotePath);
    }
    assert.equal(fake.remote.has(device.REMOTE_TEMP), false);
    assert.equal(fake.remote.has(device.REMOTE_STAGING), false);
  } finally {
    cleanupFixtureFile(pathname);
  }
});

test("备份 DB/WAL/SHM 过程中失败也恢复已移动的文件", async () => {
  const pathname = fixtureFile();
  const fake = harness({ failBackupMove: true });
  try {
    await assert.rejects(
      device.injectFixture(pathname, {
        runAdb: fake.runAdb,
        runAdbResult: fake.runAdbResult,
        ensureSingleDevice: fake.ensureSingleDevice,
        readPid: fake.readPid,
        pullStaged: fake.pullStaged,
        verifyLocal: async () => ({ ok: true }),
      }),
      /backup rename failure/,
    );
    for (const remotePath of [device.REMOTE_APP_DB, device.REMOTE_APP_DB_WAL, device.REMOTE_APP_DB_SHM]) {
      assert.equal(fake.remote.has(remotePath), true, remotePath);
    }
    assert.equal(fake.remote.has(device.REMOTE_BACKUP), false);
    assert.equal(fake.remote.has(device.REMOTE_TEMP), false);
    assert.equal(fake.remote.has(device.REMOTE_STAGING), false);
  } finally {
    cleanupFixtureFile(pathname);
  }
});

test("Release 包与非目标 serial 在任何 adb 操作前拒绝", async () => {
  const pathname = fixtureFile();
  const fake = harness();
  try {
    await assert.rejects(
      device.injectFixture(pathname, {
        packageName: "com.vcp.avatar",
        runAdb: fake.runAdb,
        verifyLocal: async () => ({ ok: true }),
      }),
      /只允许 debug 包/,
    );
    await assert.rejects(
      device.verifyDeviceFixture({ serial: "other-device", runAdb: fake.runAdb }),
      /只允许设备 serial/,
    );
    assert.equal(fake.calls.length, 0);
  } finally {
    cleanupFixtureFile(pathname);
  }
});

test("设备 verify 只信任 READY 后 Rust FTS 状态和六项语义查询，不 pull 裸主库", async () => {
  const fake = harness();
  const result = await device.verifyDeviceFixture({
    runAdb: fake.runAdb,
    ensureSingleDevice: fake.ensureSingleDevice,
    readPid: () => 9_001,
    readRuntimeStatus: async () => ({ status: runtimeStatus(), semanticChecks: semanticChecks() }),
  });
  assert.equal(result.ok, true);
  assert.equal(result.runtimeStatusVerified, true);
  assert.equal(result.liveTopicRowCount, 1_800);
  assert.equal(result.deviceFixtureVerified, true);
  assert.equal(fake.calls.some(([, args]) => args[0] === "pull"), false);
});

test("运行态 CDP 读取先等 READY，再读取 status 和无原词 evidence 的语义结果", async () => {
  const calls = [];
  const runtime = {
    cdp: {
      invoke: async (command) => {
        calls.push(command);
        if (command === "get_fts_index_status") return runtimeStatus();
        return { results: command === "search_messages_fts" ? [{}] : [] };
      },
    },
    close: async () => calls.push("close"),
  };
  const result = await device.readRustRuntimeStatus({
    reconnect: async () => runtime,
    waitReady: async () => calls.push("ready"),
  });
  assert.equal(result.status.liveCount, 50_000);
  assert.deepEqual(calls.slice(0, 3), ["ready", "get_fts_index_status", "search_messages_fts"]);
  assert.equal(result.semanticChecks[0].caseId, "default-zh-1");
  assert.equal(result.semanticChecks[0].query, undefined);
  assert.equal(calls.at(-1), "close");
});

test("设备 cleanup 聚合 restart、remote DB、temp、staging 的全部失败", async () => {
  const calls = [];
  const runAdb = (args) => {
    calls.push(args);
    if (args[0] === "shell" && args[1] === "getprop") return device.TARGET_AVD;
    if (args[0] === "shell" && args[1] === "run-as" && args[3] === "kill") {
      throw new Error("kill failure");
    }
    throw new Error(`cleanup failure ${args.join(" ")}`);
  };
  await assert.rejects(
    device.cleanupDebugFixture({
      runAdb,
      ensureSingleDevice: () => ({ serial: device.TARGET_SERIAL }),
      readPid: () => 8_001,
    }),
    (error) => {
      assert.match(error.message, /restart/);
      assert.match(error.message, /remote-database/);
      assert.match(error.message, /remote-temp/);
      assert.match(error.message, /remote-staging/);
      return true;
    },
  );
});
