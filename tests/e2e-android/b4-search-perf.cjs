"use strict";

const metrics = require("./b4-search-perf-metrics.cjs");
const runner = require("./b4-search-perf-runner.cjs");
const fixtureCli = require("./b4-search-perf-fixture-cli.cjs");
const { redactText, sanitizeDto } = require("./b4-search-perf-evidence-schema.cjs");

const SCHEMA = "vcp.android.b4.search-performance.v1";

function writeJson(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

function argumentFailure(error) {
  return {
    schema: SCHEMA,
    ok: false,
    generatedAt: new Date().toISOString(),
    package: runner.EXPECTED_DEBUG_PACKAGE,
    mode: "debug-only",
    failures: [metrics.makeFailure("arguments", redactText(metrics.errorText(error), []))],
  };
}

function helpReport() {
  return {
    schema: SCHEMA,
    ok: true,
    mode: "help",
    package: runner.EXPECTED_DEBUG_PACKAGE,
    usage: metrics.usage(),
  };
}

function uncaughtFailure(error) {
  return {
    schema: SCHEMA,
    ok: false,
    generatedAt: new Date().toISOString(),
    package: runner.EXPECTED_DEBUG_PACKAGE,
    mode: "debug-only",
    failures: [metrics.makeFailure("uncaught", redactText(metrics.errorText(error), []))],
  };
}

function safeFixtureFailure(error, options) {
  const payload = error?.fixturePayload && typeof error.fixturePayload === "object"
    ? error.fixturePayload
    : {};
  const rawFailures = [
    ...(Array.isArray(payload.failures) ? payload.failures : []),
    ...(Array.isArray(error?.cleanupFailures) ? error.cleanupFailures : []),
  ];
  const failures = rawFailures
    .map((failure) => metrics.makeFailure(
      typeof failure?.phase === "string" ? failure.phase : "fixture",
      redactText(typeof failure?.message === "string" ? failure.message : "fixture 命令失败", options.queries),
    ));
  if (failures.length === 0) {
    failures.push(metrics.makeFailure("fixture", redactText(metrics.errorText(error), options.queries)));
  }
  const residualCount = rawFailures.reduce(
    (count, failure) => count + (Array.isArray(failure?.residualPaths) ? failure.residualPaths.length : 0),
    Array.isArray(payload.residualPaths) ? payload.residualPaths.length : 0,
  );
  const fixture = sanitizeDto({
    ...payload,
    command: options.fixture,
    ok: false,
    residualCount,
    error: redactText(metrics.errorText(error), options.queries),
  }, "fixture") || { command: options.fixture, ok: false };
  return {
    schema: SCHEMA,
    ok: false,
    generatedAt: new Date().toISOString(),
    package: runner.EXPECTED_DEBUG_PACKAGE,
    mode: "fixture",
    fixture,
    failures,
  };
}

function fixtureReport(result, options) {
  const operationOk = result?.ok === true;
  const fixture = sanitizeDto({
    ...(result && typeof result === "object" ? result : {}),
    command: options.fixture,
    ok: operationOk,
  }, "fixture") || { command: options.fixture, ok: false };
  return {
    schema: SCHEMA,
    ok: fixture.ok === true,
    generatedAt: new Date().toISOString(),
    package: runner.EXPECTED_DEBUG_PACKAGE,
    mode: "fixture",
    fixture,
    failures: fixture.ok === true ? [] : [metrics.makeFailure("fixture", "fixture 命令失败")],
  };
}

async function main(argv = process.argv.slice(2)) {
  let options;
  try {
    options = metrics.parseArgs(argv);
  } catch (error) {
    writeJson(argumentFailure(error));
    return 1;
  }
  if (options.help) {
    writeJson(helpReport());
    return 0;
  }

  if (options.fixture) {
    try {
      const result = await fixtureCli.fixtureCommand(options.fixture, options);
      const report = fixtureReport(result, options);
      writeJson(report);
      return report.ok ? 0 : 1;
    } catch (error) {
      const report = safeFixtureFailure(error, options);
      writeJson(report);
      return 1;
    }
  }

  let state = runner.initialState();
  let runtimeError = null;
  try {
    state = await runner.runBenchmark(options);
  } catch (error) {
    runtimeError = error;
  }
  const report = metrics.buildReport(options, state, runtimeError);
  writeJson(report);
  return report.ok ? 0 : 1;
}

if (require.main === module) {
  main()
    .then((code) => {
      process.exitCode = Number.isInteger(code) ? code : 1;
    })
    .catch((error) => {
      writeJson(uncaughtFailure(error));
      process.exitCode = 1;
    });
}

module.exports = {
  DEFAULT_SYNTHETIC_QUERIES: metrics.DEFAULT_SYNTHETIC_QUERIES,
  INDEX_STATUS_CDP_TIMEOUT_MS: metrics.INDEX_STATUS_CDP_TIMEOUT_MS,
  REBUILD_CDP_TIMEOUT_MS: metrics.REBUILD_CDP_TIMEOUT_MS,
  THRESHOLDS: metrics.THRESHOLDS,
  buildReport: metrics.buildReport,
  fixtureReport,
  findMemoryPeak: metrics.findMemoryPeak,
  main,
  memoryDelta: metrics.memoryDelta,
  memoryPeakDelta: metrics.memoryPeakDelta,
  parseArgs: metrics.parseArgs,
  parseMeminfoMetric: metrics.parseMeminfoMetric,
  percentile: metrics.percentile,
  summarizeDurations: metrics.summarizeDurations,
};
