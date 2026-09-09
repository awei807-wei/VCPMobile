"use strict";

const { DEFAULT_SYNTHETIC_QUERIES } = require("./b4-search-perf-fixture.cjs");
const { TARGET_SERIAL } = require("./b4-search-perf-device.cjs");

const DEFAULT_SAMPLE_COUNT = 10;
const FIXTURE_COMMANDS = new Set(["prepare", "inject", "verify", "cleanup"]);

function usage() {
  return [
    "用法: node tests/e2e-android/b4-search-perf.cjs [选项]",
    "",
    "性能选项:",
    `  --samples <number>  每个阶段的计时样本数（默认 ${DEFAULT_SAMPLE_COUNT}）`,
    "  --query <text>      搜索词，可重复；未验证时默认标为 unverified-default",
    "",
    "fixture 选项:",
    "  --fixture <command> prepare|inject|verify|cleanup",
    "  --fixture-path <path> 本地 SQLite fixture 路径",
    `  --serial <serial>   设备 serial（仅允许 ${TARGET_SERIAL}）`,
    "  --device            对 Debug 应用执行 verify/cleanup 设备操作",
    "  --keep-fixture      显式保留本轮生成/注入的本地 fixture",
    "  --allow-small-fixture 允许显式覆盖默认 fixture 规模",
    "  --topics <number>   fixture topic 数",
    "  --messages <number> fixture live message 数",
    "  --min-content-bytes <number> fixture 最小解码正文字节数",
    "  --overwrite         prepare 时显式替换已有 fixture",
    "  --help              输出帮助 JSON",
  ].join("\n");
}

function positiveInteger(value, flag, maximum = 1000) {
  if (!/^\d+$/.test(value)) throw new Error(`${flag} 必须是正整数`);
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1 || parsed > maximum) {
    throw new Error(`${flag} 必须在 1 到 ${maximum} 之间`);
  }
  return parsed;
}

function nonNegativeInteger(value, flag, maximum = Number.MAX_SAFE_INTEGER) {
  if (!/^\d+$/.test(value)) throw new Error(`${flag} 必须是非负整数`);
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0 || parsed > maximum) {
    throw new Error(`${flag} 超出安全整数范围`);
  }
  return parsed;
}

function queryValue(value) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error("--query 不能为空");
  }
  const query = value.trim();
  if (query.length > 128 || Buffer.byteLength(query, "utf8") > 512) {
    throw new Error("--query 超过搜索关键词长度限制");
  }
  return query;
}

function requireValue(argv, index, argument) {
  const value = argv[index + 1];
  if (typeof value !== "string" || value.startsWith("--")) {
    throw new Error(`${argument} 缺少参数值`);
  }
  return value;
}

function parseNamedArgument(argument) {
  const separator = argument.indexOf("=");
  if (separator < 0) return null;
  return [argument.slice(0, separator), argument.slice(separator + 1)];
}

function assignOption(options, argument, value) {
  if (argument === "--samples") options.samples = positiveInteger(value, argument);
  else if (argument === "--query") options.queries.push(queryValue(value));
  else if (argument === "--fixture") {
    if (!FIXTURE_COMMANDS.has(value)) throw new Error("不支持 fixture 命令");
    options.fixture = value;
  } else if (argument === "--fixture-path") options.fixturePath = value;
  else if (argument === "--serial") {
    if (value !== TARGET_SERIAL) throw new Error(`--serial 仅允许 ${TARGET_SERIAL}`);
    options.serial = value;
  } else if (argument === "--topics") options.topics = positiveInteger(value, argument, 2_000_000);
  else if (argument === "--messages") options.messages = positiveInteger(value, argument, 5_000_000);
  else if (argument === "--min-content-bytes") {
    options.minContentBytes = nonNegativeInteger(value, argument);
  } else throw new Error("未知参数");
}

function assignBooleanOption(options, argument) {
  if (argument === "--keep-fixture") options.keepFixture = true;
  else if (argument === "--allow-small-fixture") options.allowSmallFixture = true;
  else if (argument === "--overwrite") options.overwrite = true;
  else if (argument === "--device") options.device = true;
  else return false;
  return true;
}

function parseArgs(argv) {
  const options = {
    samples: DEFAULT_SAMPLE_COUNT,
    queries: [],
    help: false,
    fixture: null,
    fixturePath: null,
    serial: TARGET_SERIAL,
    keepFixture: false,
    allowSmallFixture: false,
    overwrite: false,
    device: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--help" || argument === "-h") {
      options.help = true;
      continue;
    }
    if (assignBooleanOption(options, argument)) continue;
    const named = parseNamedArgument(argument);
    if (named) {
      assignOption(options, named[0], named[1]);
      continue;
    }
    if ([
      "--samples",
      "--query",
      "--fixture",
      "--fixture-path",
      "--serial",
      "--topics",
      "--messages",
      "--min-content-bytes",
    ].includes(argument)) {
      assignOption(options, argument, requireValue(argv, index, argument));
      index += 1;
      continue;
    }
    throw new Error("未知参数");
  }
  if (options.queries.length === 0) {
    options.queries = [...DEFAULT_SYNTHETIC_QUERIES];
    options.querySource = "unverified-default";
  } else {
    options.querySource = "cli";
  }
  return options;
}

module.exports = {
  DEFAULT_SAMPLE_COUNT,
  FIXTURE_COMMANDS,
  parseArgs,
  usage,
};
