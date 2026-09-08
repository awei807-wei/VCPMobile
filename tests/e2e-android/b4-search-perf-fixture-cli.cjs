"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const { randomBytes } = require("crypto");
const { spawnSync } = require("child_process");
const {
  REQUIRED_CORPUS,
} = require("./b4-search-perf-fixture.cjs");
const device = require("./b4-search-perf-device.cjs");

const FIXTURE_SCHEMA = "vcp.android.b4.search-fixture.v1";
const FIXTURE_SCRIPT = path.join(__dirname, "scripts", "b4_fixture.py");

function errorText(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim().slice(0, 500) || "fixture 命令失败";
}

function fixturePath(value, flag = "--fixture-path") {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${flag} 不能为空`);
  }
  const resolved = path.resolve(value);
  if (fs.existsSync(resolved) && fs.statSync(resolved).isDirectory()) {
    throw new Error(`${flag} 不能指向目录`);
  }
  return resolved;
}

function createTempFixturePath() {
  return path.join(
    os.tmpdir(),
    `vcpmobile-b4-${process.pid}-${randomBytes(8).toString("hex")}.db`,
  );
}

function pythonExecutable() {
  return process.env.B4_PYTHON || "python3";
}

function readJsonOutput(stdout) {
  const lines = String(stdout || "")
    .trim()
    .split(/\r?\n/)
    .filter(Boolean);
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    try {
      return JSON.parse(lines[index]);
    } catch {
      // argparse or a host wrapper may have emitted a diagnostic line first.
    }
  }
  throw new Error("fixture Python 输出不是 JSON");
}

function fixtureOperationError(message, payload = null) {
  const error = new Error(errorText(message));
  if (payload && typeof payload === "object") error.fixturePayload = payload;
  return error;
}

function runPython(command, args, dependencies = {}) {
  const spawn = dependencies.spawnSync || spawnSync;
  const result = spawn(pythonExecutable(), [FIXTURE_SCRIPT, command, ...args], {
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  let payload;
  try {
    payload = readJsonOutput(result.stdout);
  } catch (error) {
    const detail = String(result.stderr || "").trim();
    throw fixtureOperationError(
      `${errorText(error)}${detail ? `: ${detail.slice(0, 400)}` : ""}`,
    );
  }
  if (result.status !== 0 || payload?.ok !== true) {
    throw fixtureOperationError(
      payload?.error || result.stderr || "fixture verify 未通过",
      payload,
    );
  }
  if (payload.schema !== FIXTURE_SCHEMA) {
    throw fixtureOperationError(`fixture schema 不匹配: ${payload.schema || "empty"}`, payload);
  }
  return payload;
}

function sizeArgs(options = {}) {
  const corpus = options.corpus || {};
  const topics = corpus.topics ?? options.topics ?? REQUIRED_CORPUS.topics;
  const messages = corpus.messages ?? options.messages ?? REQUIRED_CORPUS.messages;
  const minBytes =
    corpus.minDecodedContentBytes ??
    options.minContentBytes ??
    options.minDecodedContentBytes ??
    REQUIRED_CORPUS.minDecodedContentBytes;
  return {
    topics: String(topics),
    messages: String(messages),
    minBytes: String(minBytes),
    allowSmall: options.allowSmallFixture === true,
  };
}

function appendSizeArgs(args, options) {
  const values = sizeArgs(options);
  args.push("--topics", values.topics, "--messages", values.messages);
  args.push("--min-content-bytes", values.minBytes);
  if (values.allowSmall) args.push("--allow-small-fixture");
}

function runFixturePython(command, options = {}, dependencies = {}) {
  const args = [];
  if (command === "prepare") args.push("--output", fixturePath(options.fixturePath));
  else if (command === "verify") args.push("--input", fixturePath(options.fixturePath));
  else if (command === "cleanup") args.push("--input", fixturePath(options.fixturePath));
  else throw new Error(`不支持 fixture 命令: ${command}`);
  if (command !== "cleanup") appendSizeArgs(args, options);
  if (command === "prepare" && options.overwrite === true) args.push("--overwrite");
  return runPython(command, args, dependencies);
}

function cleanupLocalFixture(fixturePathname, dependencies = {}) {
  return runFixturePython(
    "cleanup",
    { fixturePath: fixturePathname },
    dependencies,
  );
}

function mustFixturePath(options = {}) {
  if (!options.fixturePath) throw new Error("该 fixture 命令需要 --fixture-path");
  return fixturePath(options.fixturePath);
}

function prepareFixture(options = {}, dependencies = {}) {
  const pathname = options.fixturePath
    ? fixturePath(options.fixturePath)
    : createTempFixturePath();
  const result = runFixturePython(
    "prepare",
    { ...options, fixturePath: pathname },
    dependencies,
  );
  return { ...result, fixturePath: pathname };
}

function verifyFixture(options = {}, dependencies = {}) {
  return runFixturePython(
    "verify",
    { ...options, fixturePath: mustFixturePath(options) },
    dependencies,
  );
}

async function injectFixture(options = {}, dependencies = {}) {
  const pathname = mustFixturePath(options);
  const verified = verifyFixture(options, dependencies);
  const injected = await device.injectFixture(pathname, {
    ...options,
    ...dependencies,
    verifyLocal: () => verified,
  });
  return { ...verified, ...injected, command: "inject" };
}

async function verifyDeviceFixture(options = {}, dependencies = {}) {
  const verification = await device.verifyDeviceFixture({
    ...options,
    ...dependencies,
    verifyLocal: (localPath) =>
      verifyFixture({ ...options, fixturePath: localPath }, dependencies),
  });
  return verification;
}

async function cleanupFixture(options = {}, dependencies = {}) {
  const reports = [];
  const failures = [];
  if (options.fixturePath) {
    try {
      reports.push(await cleanupLocalFixture(mustFixturePath(options), dependencies));
    } catch (error) {
      failures.push({ phase: "local", message: errorText(error), payload: error.fixturePayload });
    }
  }
  if (options.device === true) {
    try {
      reports.push(await device.cleanupDebugFixture({ ...options, ...dependencies }));
    } catch (error) {
      failures.push({ phase: "device", message: errorText(error), payload: error.fixturePayload });
    }
  }
  if (failures.length) {
    const error = fixtureOperationError(
      "fixture cleanup 失败",
      {
        schema: FIXTURE_SCHEMA,
        ok: false,
        command: "cleanup",
        failures: failures.map(({ phase, message }) => ({ phase, message })),
        residualPaths: failures.flatMap((failure) =>
          Array.isArray(failure.payload?.residualPaths)
            ? failure.payload.residualPaths
            : [],
        ),
        reports: reports.map((report) => ({
          command: report?.command,
          ok: report?.ok === true,
          residualPaths: Array.isArray(report?.residualPaths) ? report.residualPaths : [],
        })),
      },
    );
    error.cleanupFailures = failures;
    throw error;
  }
  if (reports.length === 1) return reports[0];
  if (reports.length > 1) {
    return {
      ...reports[0],
      ...reports[reports.length - 1],
      command: "cleanup",
      ok: reports.every((report) => report?.ok === true),
      cleaned: reports.every((report) => report?.cleaned === true),
    };
  }
  throw new Error("cleanup 至少需要 --fixture-path，或显式 --device");
}

async function fixtureCommand(command, options = {}, dependencies = {}) {
  switch (command) {
    case "prepare":
      return prepareFixture(options, dependencies);
    case "inject":
      return injectFixture(options, dependencies);
    case "verify":
      return options.device === true
        ? verifyDeviceFixture(options, dependencies)
        : verifyFixture(options, dependencies);
    case "cleanup":
      return cleanupFixture(options, dependencies);
    default:
      throw new Error(`不支持 fixture 命令: ${command}`);
  }
}

module.exports = {
  FIXTURE_SCHEMA,
  FIXTURE_SCRIPT,
  cleanupFixture,
  cleanupLocalFixture,
  createTempFixturePath,
  fixtureCommand,
  fixtureOperationError,
  fixturePath,
  injectFixture,
  prepareFixture,
  runFixturePython,
  verifyDeviceFixture,
  verifyFixture,
};
