"use strict";

const { codedError } = require("./helper-stream-e2e-support.cjs");
const {
  channelInvokeExpression,
  cleanupHarnessStateExpression,
} = require("./helper-stream-e2e-channel.cjs");
const { cleanupResources } = require("./helper-stream-e2e-cleanup.cjs");
const { sanitizeEvidence } = require("./helper-stream-e2e-evidence.cjs");
const { runE2e, summarizeRecovery } = require("./helper-stream-e2e-flow.cjs");

const DEFAULT_TIMEOUT_MS = 30_000;
const SYNTHETIC_SEARCH_QUERY = "全";

function positiveInteger(value, flag) {
  if (!/^\d+$/.test(value)) throw codedError("ARGUMENT_INVALID", `${flag} 无效`);
  const number = Number(value);
  if (!Number.isSafeInteger(number) || number < 1) {
    throw codedError("ARGUMENT_INVALID", `${flag} 无效`);
  }
  return number;
}

function usage() {
  return {
    command: "node tests/e2e-android/helper-stream-e2e-runner.cjs",
    required: "--identity-json 或完整 ownerType/ownerId/topicId，或 --search-query",
    options: [
      "--identity-json <json>",
      "--owner-type <agent|group> --owner-id <id> --topic-id <id>",
      `--search-query <synthetic query>（默认 ${SYNTHETIC_SEARCH_QUERY}）`,
      `--timeout-ms <number>（默认 ${DEFAULT_TIMEOUT_MS}）`,
      "--evidence <path>",
      "--no-launch",
    ],
  };
}

function parseArgs(argv) {
  const options = {
    ownerType: undefined,
    ownerId: undefined,
    topicId: undefined,
    identityJson: undefined,
    searchQuery: undefined,
    timeoutMs: DEFAULT_TIMEOUT_MS,
    evidencePath: undefined,
    launch: true,
    help: false,
  };
  const valueFlags = new Map([
    ["--owner-type", "ownerType"],
    ["--owner-id", "ownerId"],
    ["--topic-id", "topicId"],
    ["--identity-json", "identityJson"],
    ["--search-query", "searchQuery"],
    ["--timeout-ms", "timeoutMs"],
    ["--evidence", "evidencePath"],
  ]);
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--help" || argument === "-h") {
      options.help = true;
      continue;
    }
    if (argument === "--no-launch") {
      options.launch = false;
      continue;
    }
    const key = valueFlags.get(argument);
    if (!key) throw codedError("ARGUMENT_INVALID", "未知参数");
    const value = argv[index + 1];
    if (typeof value !== "string" || value.startsWith("--")) {
      throw codedError("ARGUMENT_INVALID", "参数缺少值");
    }
    index += 1;
    options[key] = key === "timeoutMs" ? positiveInteger(value, argument) : value;
  }
  if (options.searchQuery === "") options.searchQuery = undefined;
  if (!options.searchQuery && !options.identityJson && options.ownerType === undefined) {
    options.searchQuery = SYNTHETIC_SEARCH_QUERY;
  }
  return options;
}

async function main(argv = process.argv.slice(2)) {
  let options;
  try {
    options = parseArgs(argv);
    if (options.help) {
      console.log(JSON.stringify({ ok: true, usage: usage() }));
      return 0;
    }
    const evidence = await runE2e(options);
    console.log(JSON.stringify(evidence));
    return evidence.ok ? 0 : 1;
  } catch (error) {
    const evidence = sanitizeEvidence({ ok: false, failure: error });
    console.log(JSON.stringify(evidence));
    return 1;
  }
}

if (require.main === module) {
  main().then((code) => {
    process.exitCode = code;
  }).catch(() => {
    process.exitCode = 1;
  });
}

module.exports = {
  cleanupHarnessStateExpression,
  cleanupResources,
  channelInvokeExpression,
  main,
  parseArgs,
  run: runE2e,
  runE2e,
  summarizeRecovery,
};
