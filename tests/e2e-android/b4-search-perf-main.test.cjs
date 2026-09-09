"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const fixtureCli = require("./b4-search-perf-fixture-cli.cjs");
const runner = require("./b4-search-perf-runner.cjs");
const { main } = require("./b4-search-perf.cjs");

async function captureMain(argv) {
  const chunks = [];
  const originalWrite = process.stdout.write;
  process.stdout.write = (chunk) => {
    chunks.push(String(chunk));
    return true;
  };
  try {
    const code = await main(argv);
    return { code, report: JSON.parse(chunks.join("")) };
  } finally {
    process.stdout.write = originalWrite;
  }
}

test("四个 fixture 子命令优先 dispatch，绝不进入 benchmark runner", async () => {
  const originalFixtureCommand = fixtureCli.fixtureCommand;
  const originalRunBenchmark = runner.runBenchmark;
  const commands = [];
  let benchmarkCalls = 0;
  fixtureCli.fixtureCommand = async (command) => {
    commands.push(command);
    return { command, ok: true, prepared: command === "prepare" };
  };
  runner.runBenchmark = async () => {
    benchmarkCalls += 1;
    throw new Error("benchmark must not run for fixture command");
  };
  try {
    for (const command of ["prepare", "verify", "inject", "cleanup"]) {
      const result = await captureMain(["--fixture", command, "--fixture-path", "/tmp/b4-fixture.db"]);
      assert.equal(result.code, 0);
      assert.equal(result.report.mode, "fixture");
      assert.equal(result.report.ok, true);
      assert.equal(result.report.fixture.command, command);
    }
  } finally {
    fixtureCli.fixtureCommand = originalFixtureCommand;
    runner.runBenchmark = originalRunBenchmark;
  }
  assert.deepEqual(commands, ["prepare", "verify", "inject", "cleanup"]);
  assert.equal(benchmarkCalls, 0);
});

test("fixture 异常出口只输出白名单 DTO，并脱敏查询词、路径和 serial", async () => {
  const originalFixtureCommand = fixtureCli.fixtureCommand;
  fixtureCli.fixtureCommand = async () => {
    const error = new Error(
      "fixture=/tmp/private-b4.db query=secret-query serial=secret-device",
    );
    error.fixturePayload = {
      schema: "vcp.android.b4.search-fixture.v1",
      ok: false,
      command: "verify",
      residualPaths: ["/tmp/private-b4.db-wal"],
      unknown: "secret-query",
    };
    throw error;
  };
  try {
    const result = await captureMain([
      "--fixture",
      "verify",
      "--fixture-path",
      "/tmp/private-b4.db",
      "--query",
      "secret-query",
    ]);
    const serialized = JSON.stringify(result.report);
    assert.equal(result.code, 1);
    assert.equal(serialized.includes("secret-query"), false);
    assert.equal(serialized.includes("private-b4.db"), false);
    assert.equal(serialized.includes("secret-device"), false);
    assert.equal(result.report.fixture.unknown, undefined);
    assert.equal(result.report.fixture.residualCount, 1);
  } finally {
    fixtureCli.fixtureCommand = originalFixtureCommand;
  }
});

test("参数异常出口不回显原始 CLI 值", async () => {
  const result = await captureMain([
    "--unknown=private-cli-value.db",
    "--query",
    "query-secret-value",
  ]);
  const serialized = JSON.stringify(result.report);
  assert.equal(result.code, 1);
  assert.equal(serialized.includes("private-cli-value.db"), false);
  assert.equal(serialized.includes("query-secret-value"), false);
});
