"use strict";

const crypto = require("node:crypto");
const { DEBUG_PACKAGE, runAdb } = require("./scripts/adb-env.cjs");

const SSE_PATHS = Object.freeze([
  "/v1/chat/completions",
  "/v1/chatvcp/completions",
]);
const OWNER_TYPES = new Set(["agent", "group"]);
const TEST_ID = "helper-e2e-01";
const DEBUG_SENTINEL_KEY = "android-helper-e2e-sentinel";

function codedError(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function requireText(value, code) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw codedError(code, "身份字段无效");
  }
  if (value.length > 256 || value.includes("\0")) {
    throw codedError(code, "身份字段超出限制");
  }
  return value;
}

function validateIdentity(value) {
  if (!isRecord(value)) throw codedError("IDENTITY_INVALID", "身份必须是对象");
  const ownerType = requireText(value.ownerType, "IDENTITY_INVALID");
  if (!OWNER_TYPES.has(ownerType)) {
    throw codedError("IDENTITY_INVALID", "ownerType 不受支持");
  }
  return {
    ownerType,
    ownerId: requireText(value.ownerId, "IDENTITY_INVALID"),
    topicId: requireText(value.topicId, "IDENTITY_INVALID"),
  };
}

function readSearchField(result, camel, snake) {
  return result?.[camel] ?? result?.[snake];
}

function identityFromSearchResult(raw) {
  const candidates = Array.isArray(raw?.results)
    ? raw.results
    : Array.isArray(raw)
      ? raw
      : isRecord(raw?.result)
        ? [raw.result]
        : [raw];
  const result = candidates[0];
  if (!isRecord(result)) {
    throw codedError("SEARCH_RESULT_INVALID", "搜索结果为空");
  }
  return validateIdentity({
    ownerType: readSearchField(result, "ownerType", "owner_type"),
    ownerId: readSearchField(result, "ownerId", "owner_id"),
    topicId: readSearchField(result, "topicId", "topic_id"),
  });
}

function identityFromJson(raw) {
  let value;
  try {
    value = JSON.parse(raw);
  } catch {
    throw codedError("IDENTITY_JSON_INVALID", "身份 JSON 无效");
  }
  return validateIdentity(value);
}

function resolveIdentityInput(options) {
  const supplied = [options.ownerType, options.ownerId, options.topicId];
  if (supplied.some((item) => item !== undefined)) {
    if (supplied.some((item) => item === undefined)) {
      throw codedError("IDENTITY_INCOMPLETE", "必须同时提供完整身份");
    }
    return validateIdentity({
      ownerType: options.ownerType,
      ownerId: options.ownerId,
      topicId: options.topicId,
    });
  }
  if (typeof options.identityJson === "string") {
    return identityFromJson(options.identityJson);
  }
  return null;
}

function createMessageId(randomBytes = crypto.randomBytes) {
  const suffix = randomBytes(12).toString("hex");
  if (!/^[0-9a-f]{24}$/.test(suffix)) {
    throw codedError("MESSAGE_ID_INVALID", "随机消息 ID 生成失败");
  }
  return `android-helper-e2e-${suffix}`;
}

function validatePort(value) {
  if (!Number.isSafeInteger(value) || value < 1 || value > 65_535) {
    throw codedError("PORT_INVALID", "端口无效");
  }
  return value;
}

function assertDebugPackage(packageName) {
  if (packageName !== DEBUG_PACKAGE) {
    throw codedError("DEBUG_PACKAGE_REQUIRED", "仅允许 Android Debug 包");
  }
  return packageName;
}

function addPreciseReverse(localPort, dependencies = {}) {
  validatePort(localPort);
  const adb = dependencies.runAdb || runAdb;
  adb(["reverse", `tcp:${localPort}`, `tcp:${localPort}`], {
    allowFailure: false,
  });
  return { localPort, remotePort: localPort, created: true };
}

function removePreciseReverse(binding, dependencies = {}) {
  if (!binding?.created) return false;
  validatePort(binding.localPort);
  const adb = dependencies.runAdb || runAdb;
  adb(["reverse", "--remove", `tcp:${binding.localPort}`], {
    allowFailure: true,
  });
  return true;
}

module.exports = {
  DEBUG_SENTINEL_KEY,
  DEBUG_PACKAGE,
  OWNER_TYPES,
  SSE_PATHS,
  TEST_ID,
  addPreciseReverse,
  assertDebugPackage,
  codedError,
  createMessageId,
  identityFromJson,
  identityFromSearchResult,
  removePreciseReverse,
  resolveIdentityInput,
  validateIdentity,
  validatePort,
};
