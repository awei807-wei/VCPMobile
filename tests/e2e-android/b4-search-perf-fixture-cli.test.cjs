"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const fixtureCli = require("./b4-search-perf-fixture-cli.cjs");
const device = require("./b4-search-perf-device.cjs");

const SCHEMA = fixtureCli.FIXTURE_SCHEMA;

function fixturePath() {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "b4-fixture-cli-test-"));
  const pathname = path.join(directory, "fixture.db");
  fs.writeFileSync(pathname, "fixture");
  return pathname;
}

function cleanupPath(pathname) {
  try {
    fs.unlinkSync(pathname);
  } catch {}
  try {
    fs.rmdirSync(path.dirname(pathname));
  } catch {}
}

function pythonResult(payload, status = 0) {
  return {
    status,
    stdout: `${JSON.stringify(payload)}\n`,
    stderr: "",
  };
}

function successfulPayload(command) {
  return {
    schema: SCHEMA,
    ok: true,
    command,
    verified: command === "verify",
    prepared: command === "prepare",
    cleaned: command === "cleanup",
  };
}

test("prepare 成功返回生成路径且不替调用方清理 fixture", () => {
  const pathname = fixturePath();
  let cleanupCalls = 0;
  try {
    const result = fixtureCli.prepareFixture(
      { fixturePath: pathname },
      {
        spawnSync: () => pythonResult(successfulPayload("prepare")),
      },
    );
    assert.equal(result.ok, true);
    assert.equal(result.fixturePath, pathname);
    assert.equal(fs.existsSync(pathname), true);
    assert.equal(cleanupCalls, 0);
  } finally {
    cleanupPath(pathname);
  }
});

test("inject 成功或失败都保留用户传入的本地 fixture 路径", async () => {
  const pathname = fixturePath();
  const originalInject = device.injectFixture;
  const calls = [];
  try {
    device.injectFixture = async (source) => {
      calls.push(source);
      return { injected: true, stagedFixtureVerified: true };
    };
    const success = await fixtureCli.injectFixture(
      { fixturePath: pathname },
      { spawnSync: () => pythonResult(successfulPayload("verify")) },
    );
    assert.equal(success.ok, true);
    assert.equal(success.command, "inject");
    assert.equal(fs.existsSync(pathname), true);

    device.injectFixture = async () => {
      throw new Error("injected device failure");
    };
    await assert.rejects(
      fixtureCli.injectFixture(
        { fixturePath: pathname },
        { spawnSync: () => pythonResult(successfulPayload("verify")) },
      ),
      /injected device failure/,
    );
    assert.deepEqual(calls, [pathname]);
    assert.equal(fs.existsSync(pathname), true);
  } finally {
    device.injectFixture = originalInject;
    cleanupPath(pathname);
  }
});

test("cleanup 即使本地 Python 失败仍继续设备清理并聚合两侧错误", async () => {
  const pathname = fixturePath();
  const originalCleanup = device.cleanupDebugFixture;
  let deviceCleanupCalls = 0;
  const residual = "/tmp/private-fixture-wal";
  try {
    device.cleanupDebugFixture = async () => {
      deviceCleanupCalls += 1;
      const error = new Error("device cleanup failed");
      error.fixturePayload = {
        schema: SCHEMA,
        ok: false,
        residualPaths: [residual],
      };
      throw error;
    };
    const localFailure = {
      schema: SCHEMA,
      ok: false,
      command: "cleanup",
      error: "local cleanup failed",
      residualPaths: ["/tmp/private-fixture.db"],
    };
    await assert.rejects(
      fixtureCli.cleanupFixture(
        { fixturePath: pathname, device: true },
        { spawnSync: () => pythonResult(localFailure, 1) },
      ),
      (error) => {
        assert.match(error.message, /fixture cleanup/);
        assert.deepEqual(
          error.cleanupFailures.map((failure) => failure.phase),
          ["local", "device"],
        );
        assert.deepEqual(error.fixturePayload.residualPaths, [
          "/tmp/private-fixture.db",
          residual,
        ]);
        return true;
      },
    );
    assert.equal(deviceCleanupCalls, 1);
  } finally {
    device.cleanupDebugFixture = originalCleanup;
    cleanupPath(pathname);
  }
});

test("Python 失败保留结构化 payload 及 residualPaths", () => {
  const pathname = fixturePath();
  const payload = {
    schema: SCHEMA,
    ok: false,
    command: "verify",
    error: "content type mismatch",
    residualPaths: ["/tmp/private-fixture-shm"],
  };
  try {
    assert.throws(
      () => fixtureCli.runFixturePython(
        "verify",
        { fixturePath: pathname },
        { spawnSync: () => pythonResult(payload, 1) },
      ),
      (error) => {
        assert.deepEqual(error.fixturePayload, payload);
        return true;
      },
    );
  } finally {
    cleanupPath(pathname);
  }
});
