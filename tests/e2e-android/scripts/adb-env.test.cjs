"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const adbEnvPath = require.resolve("./adb-env.cjs");

function executableAdb(source) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "adb-env-test-"));
  const pathname = path.join(directory, "adb-stub.sh");
  fs.writeFileSync(pathname, `#!/bin/sh\n${source}\n`);
  fs.chmodSync(pathname, 0o755);
  return { directory, pathname };
}

function removeTree(directory) {
  fs.rmSync(directory, { recursive: true, force: true });
}

test("runAdbResult 暴露 status/stdout/stderr，并保留 runAdb 兼容返回值", () => {
  const previous = process.env.ADB;
  process.env.ADB = "/bin/echo";
  try {
    delete require.cache[adbEnvPath];
    const adbEnv = require("./adb-env.cjs");
    const result = adbEnv.runAdbResult([
      "adb-out",
    ]);
    assert.deepEqual(result, {
      status: 0,
      stdout: "adb-out\n",
      stderr: "",
    });
    assert.equal(adbEnv.runAdb(["legacy-out"]), "legacy-out\n");

    delete require.cache[adbEnvPath];
    process.env.ADB = "/bin/false";
    const failingAdbEnv = require("./adb-env.cjs");
    const failure = failingAdbEnv.runAdbResult([]);
    assert.equal(failure.status, 1);
    assert.equal(typeof failure.stdout, "string");
    assert.equal(typeof failure.stderr, "string");
  } finally {
    if (previous === undefined) delete process.env.ADB;
    else process.env.ADB = previous;
    delete require.cache[adbEnvPath];
  }
});

test("runAdbToFile 通过 stdout 文件描述符传输超过 32MiB 的二进制输出", () => {
  const previous = process.env.ADB;
  const outputDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "adb-output-test-"));
  const outputPath = path.join(outputDirectory, "fixture.db");
  const stub = executableAdb("dd if=/dev/zero bs=1048576 count=33 2>/dev/null");
  process.env.ADB = stub.pathname;
  try {
    delete require.cache[adbEnvPath];
    const adbEnv = require("./adb-env.cjs");
    const result = adbEnv.runAdbToFile(["exec-out", "run-as", "com.vcp.avatar.debug", "cat", "staged"], outputPath);
    assert.equal(result.status, 0);
    assert.equal(result.bytesWritten, 33 * 1024 * 1024);
    assert.equal(fs.statSync(outputPath).size, 33 * 1024 * 1024);
    const fd = fs.openSync(outputPath, "r");
    const firstByte = Buffer.alloc(1);
    try {
      assert.equal(fs.readSync(fd, firstByte, 0, firstByte.length, 0), 1);
      assert.equal(firstByte[0], 0x00);
    } finally {
      fs.closeSync(fd);
    }
  } finally {
    if (previous === undefined) delete process.env.ADB;
    else process.env.ADB = previous;
    delete require.cache[adbEnvPath];
    removeTree(stub.directory);
    removeTree(outputDirectory);
  }
});

test("二进制 adb 失败不解码或泄露 stdout，只报告状态、字节数和受限 stderr", () => {
  const previous = process.env.ADB;
  const outputDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "adb-failure-test-"));
  const outputPath = path.join(outputDirectory, "fixture.db");
  const secret = "SQLITE_BINARY_SECRET_DO_NOT_PRINT";
  const stub = executableAdb(`printf '%s' '${secret}'\nprintf '%s' 'transfer failed\\n' >&2\nexit 7`);
  process.env.ADB = stub.pathname;
  try {
    delete require.cache[adbEnvPath];
    const adbEnv = require("./adb-env.cjs");
    assert.throws(
      () => adbEnv.runAdbToFile(["exec-out", "run-as", "com.vcp.avatar.debug", "cat", "staged"], outputPath),
      (error) => {
        assert.match(error.message, /status=7/);
        assert.match(error.message, /bytes=/);
        assert.match(error.message, /transfer failed/);
        assert.equal(error.message.includes(secret), false);
        assert.equal(error.stdoutOmitted, true);
        return true;
      },
    );
    assert.throws(
      () => adbEnv.runAdbBuffer(["exec-out", "run-as", "com.vcp.avatar.debug", "cat", "staged"]),
      (error) => {
        assert.equal(error.message.includes(secret), false);
        assert.equal(error.stdoutOmitted, true);
        return true;
      },
    );
  } finally {
    if (previous === undefined) delete process.env.ADB;
    else process.env.ADB = previous;
    delete require.cache[adbEnvPath];
    removeTree(stub.directory);
    removeTree(outputDirectory);
  }
});
