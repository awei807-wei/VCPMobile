"use strict";

const { reconnectAndroidCdp } = require("./wire14/android-state.cjs");
const { waitForCoreReady } = require("./b4-search-perf-readiness.cjs");

const INDEX_STATUS_TIMEOUT_MS = 15_000;
const SEMANTIC_CASES = Object.freeze([
  ["default-zh-1", "中文1字", "全", "non-empty"],
  ["default-zh-2", "中文2字", "全局", "non-empty"],
  ["default-zh-3", "中文3+字", "三个字", "non-empty"],
  ["default-en", "英文", "English", "non-empty"],
  ["default-special", "特殊字符", "a.b", "non-empty"],
  ["default-empty", "确定空结果", "b4-no-such-token-9f3e7c", "empty"],
]);

function runtimeField(status, key, fallback) {
  return status?.[key] ?? (fallback ? status?.[fallback] : undefined);
}

function safeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function normalizeRuntimeStatus(status) {
  return {
    available: runtimeField(status, "available") === true,
    schemaValid: runtimeField(status, "schemaValid", "schema_valid") === true,
    tokenizerValid: runtimeField(status, "tokenizerValid", "tokenizer_valid") === true,
    healthy: runtimeField(status, "healthy") === true,
    topicCount: safeInteger(runtimeField(status, "topicCount", "topic_count")),
    liveTopicRowCount: safeInteger(runtimeField(status, "liveTopicRowCount", "live_topic_row_count")),
    liveCount: safeInteger(runtimeField(status, "liveCount", "live_count")),
    indexedCount: safeInteger(runtimeField(status, "indexedCount", "indexed_count")),
    missingCount: safeInteger(runtimeField(status, "missingCount", "missing_count")),
    orphanCount: safeInteger(runtimeField(status, "orphanCount", "orphan_count")),
    duplicateCount: safeInteger(runtimeField(status, "duplicateCount", "duplicate_count")),
    staleCount: safeInteger(runtimeField(status, "staleCount", "stale_count")),
    decodeErrorCount: safeInteger(runtimeField(status, "decodeErrorCount", "decode_error_count")),
    decodedContentBytes: safeInteger(runtimeField(status, "decodedContentBytes", "decoded_content_bytes")),
  };
}

function normalizeSemanticChecks(checks) {
  const byCase = new Map(
    (Array.isArray(checks) ? checks : []).map((item) => [item?.caseId, item]),
  );
  return SEMANTIC_CASES.map(([caseId, category, , expectation]) => {
    const source = byCase.get(caseId);
    return {
      caseId,
      category,
      expectation,
      nonEmpty: source?.nonEmpty === true,
      passed: source?.passed === true,
    };
  });
}

function runtimeFixtureChecks(status, semanticChecks) {
  const dto = normalizeRuntimeStatus(status);
  const checks = {
    statusAvailable: dto.available,
    schema: dto.schemaValid,
    tokenizer: dto.tokenizerValid,
    healthy: dto.healthy,
    topicsExact: dto.topicCount === 1_800,
    liveTopicRowsExact: dto.liveTopicRowCount === 1_800,
    messagesExact: dto.liveCount === 50_000,
    indexedExact: dto.indexedCount === 50_000,
    decodedContentMinimum:
      dto.decodedContentBytes !== null && dto.decodedContentBytes >= 100 * 1024 * 1024,
    zeroIntegrityErrors: [
      dto.missingCount,
      dto.orphanCount,
      dto.duplicateCount,
      dto.staleCount,
      dto.decodeErrorCount,
    ].every((value) => value === 0),
    semanticQueries:
      semanticChecks.length === SEMANTIC_CASES.length &&
      semanticChecks.every((item) => item.passed === true),
  };
  return { dto, checks, valid: Object.values(checks).every(Boolean) };
}

async function runSemanticQueries(cdp) {
  const checks = [];
  for (const [caseId, category, query, expectation] of SEMANTIC_CASES) {
    try {
      const response = await cdp.invoke("search_messages_fts", {
        filter: { query, limit: 50, sort: "time" },
      });
      const nonEmpty = Array.isArray(response?.results) && response.results.length > 0;
      checks.push({
        caseId,
        category,
        expectation,
        nonEmpty,
        passed: expectation === "empty" ? !nonEmpty : nonEmpty,
      });
    } catch {
      checks.push({ caseId, category, expectation, nonEmpty: false, passed: false });
    }
  }
  return checks;
}

async function readRustRuntimeStatus(options = {}) {
  const reconnect = options.reconnect || reconnectAndroidCdp;
  const waitReady = options.waitReady || waitForCoreReady;
  const runtime = await reconnect();
  const cdp = runtime?.cdp || runtime;
  let primary = null;
  let result = null;
  try {
    await waitReady(cdp);
    const status = await cdp.invoke(
      "get_fts_index_status",
      {},
      { timeoutMs: options.indexStatusTimeoutMs || INDEX_STATUS_TIMEOUT_MS },
    );
    result = { status, semanticChecks: await runSemanticQueries(cdp) };
  } catch (error) {
    primary = error;
  }
  let closeError = null;
  try {
    const closeResult = runtime?.close?.();
    await closeResult?.catch?.(() => {
      throw new Error("关闭运行态 CDP 连接失败");
    });
  } catch (error) {
    closeError = error;
  }
  if (primary && closeError) {
    throw new Error(`${primary.message || primary}；CDP cleanup 失败：${closeError.message}`);
  }
  if (primary) throw primary;
  if (closeError) throw closeError;
  return result;
}

module.exports = {
  INDEX_STATUS_TIMEOUT_MS,
  normalizeRuntimeStatus,
  normalizeSemanticChecks,
  readRustRuntimeStatus,
  runSemanticQueries,
  runtimeFixtureChecks,
  SEMANTIC_CASES,
};
