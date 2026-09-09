"use strict";

const { DEBUG_PACKAGE } = require("./scripts/adb-env.cjs");
const {
  CORE_READY_POLL_INTERVAL_MS,
  CORE_READY_TIMEOUT_MS,
} = require("./b4-search-perf-readiness.cjs");
const {
  COLD_RESTART_POLL_INTERVAL_MS,
  COLD_RESTART_TIMEOUT_MS,
} = require("./b4-search-perf-restart.cjs");
const { coldRestartChecks, mergeColdRestartGateFailures } = require("./b4-search-perf-gates.cjs");
const {
  DEFAULT_QUERY_SPECS,
  DEFAULT_SYNTHETIC_QUERIES,
  REQUIRED_CORPUS,
  readStatusValue,
  syntheticCorpusMissing,
  validateCorpusStatus,
  validateQuerySemantics,
} = require("./b4-search-perf-fixture.cjs");
const {
  redactFailures,
  redactText,
  sanitizeCold,
  sanitizeWarm,
  sanitizeSafeValue,
} = require("./b4-search-perf-evidence.cjs");
const reportSupport = require("./b4-search-perf-report.cjs");
const {
  findMemoryPeak,
  memoryDelta: calculateMemoryDelta,
  memoryPeakDelta: calculateMemoryPeakDelta,
  parseMeminfoMetric,
  readAndroidMemory,
  unavailableReading,
} = require("./b4-search-perf-memory.cjs");
const {
  DEFAULT_SAMPLE_COUNT,
  parseArgs,
  usage,
} = require("./b4-search-perf-cli.cjs");
const SEARCH_LIMIT = 50;
const SEARCH_SORT = "time";
const THRESHOLDS = Object.freeze({
  warmP95Ms: 300,
  coldP95Ms: 1000,
  rebuildWallClockMs: 180000,
  memoryDeltaBytes: 209715200,
});
const COLD_RESTART_STRATEGY = "debug-same-uid-sigkill";
const COLD_RESTART_STRATEGY_REASON =
  "通过 run-as 以应用 UID 直接 SIGKILL 旧 debug 进程，再用 monkey 启动新进程；避免 am force-stop 的任务和生命周期副作用，测量真实冷进程 READY 后的首次查询。";
const COLD_RESTART_STRATEGY_BOUNDARY =
  "仅适用于 debug-only 包 com.vcp.avatar.debug；每轮必须确认旧 PID 消失、新 PID 不同、应用回到前台且 CDP PID 匹配，不用于 release/生产进程。";
const REBUILD_CDP_TIMEOUT_MS = THRESHOLDS.rebuildWallClockMs + 5000;
const INDEX_STATUS_CDP_TIMEOUT_MS = 15_000;

/** Use nearest-rank so small reproducible samples do not interpolate. */
function percentile(values, probability = 0.95) {
  const clean = values
    .filter((value) => Number.isFinite(value))
    .sort((left, right) => left - right);
  if (clean.length === 0) return null;
  const rank = Math.max(1, Math.ceil(clean.length * probability));
  return clean[Math.min(clean.length - 1, rank - 1)];
}

function roundMs(value) {
  return Number(value.toFixed(3));
}

function summarizeDurations(samples) {
  const values = samples
    .filter((sample) => sample?.ok && Number.isFinite(sample.durationMs))
    .map((sample) => sample.durationMs);
  if (values.length === 0) {
    return {
      sampleCount: 0,
      minMs: null,
      medianMs: null,
      p95Ms: null,
      maxMs: null,
      meanMs: null,
    };
  }
  const sorted = [...values].sort((left, right) => left - right);
  const mean = values.reduce((sum, value) => sum + value, 0) / values.length;
  return {
    sampleCount: values.length,
    minMs: roundMs(sorted[0]),
    medianMs: roundMs(percentile(values, 0.5)),
    p95Ms: roundMs(percentile(values, 0.95)),
    maxMs: roundMs(sorted[sorted.length - 1]),
    meanMs: roundMs(mean),
  };
}

function errorText(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim().slice(0, 400) || "未知错误";
}

function validateIndexStatus(status) {
  const requiredBooleans = ["available", "schemaValid", "tokenizerValid", "healthy"];
  const booleansValid = requiredBooleans.every((key) => {
    const fallbackKey = key.replace(/[A-Z]/g, (value) => `_${value.toLowerCase()}`);
    return readStatusValue(status, key, fallbackKey) === true;
  });
  const countFields = [
    ["topicCount", "topic_count"],
    ["liveTopicRowCount", "live_topic_row_count"],
    ["liveCount", "live_count"],
    ["indexedCount", "indexed_count"],
    ["missingCount", "missing_count"],
    ["orphanCount", "orphan_count"],
    ["duplicateCount", "duplicate_count"],
    ["staleCount", "stale_count"],
    ["decodeErrorCount", "decode_error_count"],
  ];
  const countsValid = countFields.every(([key, fallbackKey]) => {
    const value = readStatusValue(status, key, fallbackKey);
    return Number.isSafeInteger(value) && value >= 0;
  });
  const healthy = booleansValid && countsValid;
  return {
    healthy,
    reason: healthy ? null : "FTS 索引不可用、不完整、计数无效或 healthy=false",
  };
}

function searchFilter(query) {
  return { query, limit: SEARCH_LIMIT, sort: SEARCH_SORT };
}

function normalizeSearchPage(raw) {
  if (!raw || typeof raw !== "object" || !Array.isArray(raw.results)) {
    throw new Error("搜索响应缺少 results 数组");
  }
  return {
    resultCount: raw.results.length,
    nextCursorPresent:
      typeof raw.nextCursor === "string" || typeof raw.next_cursor === "string",
  };
}

function memoryDelta(before, after, options = {}) {
  return calculateMemoryDelta(
    before,
    after,
    THRESHOLDS.memoryDeltaBytes,
    options,
  );
}

function memoryPeakDelta(before, peak, samples, options = {}) {
  return calculateMemoryPeakDelta(
    before,
    peak,
    samples,
    THRESHOLDS.memoryDeltaBytes,
    options,
  );
}

function makeFailure(phase, message) {
  return { phase, message };
}

function reportChecks(state, warmStats, coldStats, memory, memoryPeak) {
  const coldGates = coldRestartChecks(state.cold?.samples);
  const syntheticFixture = state.querySource === "b4-synthetic-fixture";
  const fixtureVerified = fixtureVerificationPassed(state, syntheticFixture);
  const queryValidation = state.warm?.queryValidation;
  const peak = memoryPeak ||
    memoryPeakDelta(
      state.rebuildMemoryBefore,
      state.rebuildMemoryPeak,
      state.rebuildMemorySamples,
    );
  return {
    indexBeforeHealthy: state.indexValidationBefore?.healthy === true,
    corpusBeforeValid: state.corpusValidationBefore?.valid === true,
    querySemanticsValid: syntheticFixture
      ? queryValidation?.applicable === true && queryValidation.valid === true
      : queryValidation?.valid !== false,
    fixtureVerified,
    warmP95WithinLimit:
      warmStats.p95Ms !== null && warmStats.p95Ms <= THRESHOLDS.warmP95Ms,
    coldP95WithinLimit:
      coldStats.p95Ms !== null && coldStats.p95Ms <= THRESHOLDS.coldP95Ms,
    ...coldGates,
    rebuildWithinLimit:
      state.rebuild?.ok === true &&
      state.rebuild.wallClockMs <= THRESHOLDS.rebuildWallClockMs,
    rssAvailable: memory.rssAvailable === true && peak.rssAvailable === true,
    memoryRssAvailable:
      memory.rssAvailable === true && peak.rssAvailable === true,
    memoryPidsConsistent:
      memory.pidConsistent === true && peak.pidConsistent === true,
    memoryPeakSampled: peak.peakSampled === true,
    memoryPeakWithinLimit: peak.gatePassed === true,
    memoryDeltaWithinLimit: memory.gatePassed,
    indexAfterRebuildHealthy:
      state.indexValidationAfterRebuild?.healthy === true,
  };
}

function fixtureVerificationPassed(state, required) {
  if (!required) return true;
  const fixture = state.fixtureValidation;
  const requirements = fixture?.requirements || REQUIRED_CORPUS;
  const fullScale =
    requirements.topics === REQUIRED_CORPUS.topics &&
    requirements.liveTopicRows === REQUIRED_CORPUS.liveTopicRows &&
    requirements.messages === REQUIRED_CORPUS.messages &&
    requirements.minDecodedContentBytes >= REQUIRED_CORPUS.minDecodedContentBytes;
  return fixture?.verified === true && fullScale;
}

function buildReportFailures(state, options, runtimeError, memoryPeak) {
  const failures = redactFailures(
    mergeColdRestartGateFailures(state.failures, state.cold?.samples),
    options.queries,
  );
  if (runtimeError) {
    failures.push(
      makeFailure("runtime", redactText(errorText(runtimeError), options.queries)),
    );
  }
  if (!memoryPeak.gatePassed) {
    failures.push(makeFailure("memory_peak", memoryPeak.reason));
  }
  if (
    options.querySource === "b4-synthetic-fixture" &&
    !fixtureVerificationPassed(state, true)
  ) {
    failures.push(makeFailure("corpus", "未通过本轮完整 B4 fixture 验证"));
  }
  return failures;
}

function buildReportIndex(state, options) {
  return {
    before: sanitizeSafeValue(
      state.indexBefore || null,
      options.queries,
      "index",
    ),
    afterRebuild: sanitizeSafeValue(
      state.indexAfterRebuild || null,
      options.queries,
      "index",
    ),
  };
}

function buildReport(options, state, runtimeError = null) {
  const stateWithSource = { ...state, querySource: options.querySource };
  const warmStats = state.warm?.stats || summarizeDurations([]);
  const coldStats = state.cold?.stats || summarizeDurations([]);
  const expectedPid =
    state.rebuild?.expectedPid ?? state.rebuildMemoryBefore?.pid ?? null;
  const rebuildMemory = memoryDelta(
    state.rebuildMemoryBefore,
    state.rebuildMemoryAfter,
    { requireSamePid: true, expectedPid },
  );
  const memoryPeak = memoryPeakDelta(
    state.rebuildMemoryBefore,
    state.rebuildMemoryPeak,
    state.rebuildMemorySamples,
    { expectedPid },
  );
  const checks = reportChecks(
    stateWithSource,
    warmStats,
    coldStats,
    rebuildMemory,
    memoryPeak,
  );
  const failures = buildReportFailures(stateWithSource, options, runtimeError, memoryPeak);
  const reportWarm = sanitizeWarm(state.warm, options.querySource, options.queries);
  const reportCold = sanitizeCold(state.cold, options.querySource, options.queries);
  const ok = failures.length === 0 && Object.values(checks).every(Boolean);
  return {
    schema: "vcp.android.b4.search-performance.v1",
    ok,
    generatedAt: new Date().toISOString(),
    package: DEBUG_PACKAGE,
    mode: "debug-only",
    device: sanitizeSafeValue(state.device || null, options.queries, "device"),
    configuration: reportSupport.buildReportConfiguration(options, state, {
      coldRestartStrategy: COLD_RESTART_STRATEGY,
      coldRestartStrategyReason: COLD_RESTART_STRATEGY_REASON,
      coldRestartStrategyBoundary: COLD_RESTART_STRATEGY_BOUNDARY,
      searchLimit: SEARCH_LIMIT,
      searchSort: SEARCH_SORT,
      rebuildCdpTimeoutMs: REBUILD_CDP_TIMEOUT_MS,
      indexStatusCdpTimeoutMs: INDEX_STATUS_CDP_TIMEOUT_MS,
    }),
    thresholds: { ...THRESHOLDS },
    index: buildReportIndex(state, options),
    warm: reportWarm || { warmups: [], samples: [], stats: warmStats },
    cold: reportCold || { samples: [], stats: coldStats },
    rebuild: sanitizeSafeValue(
      state.rebuild || null,
      options.queries,
      "rebuild",
    ),
    memory: sanitizeSafeValue(
      reportSupport.buildMemoryReport(state, rebuildMemory, memoryPeak, expectedPid),
      options.queries,
      "memory",
    ),
    checks,
    failures,
  };
}

module.exports = {
  DEFAULT_SAMPLE_COUNT,
  DEFAULT_QUERY_SPECS,
  DEFAULT_SYNTHETIC_QUERIES,
  COLD_RESTART_STRATEGY,
  COLD_RESTART_STRATEGY_BOUNDARY,
  COLD_RESTART_STRATEGY_REASON,
  INDEX_STATUS_CDP_TIMEOUT_MS,
  REBUILD_CDP_TIMEOUT_MS,
  SEARCH_LIMIT,
  SEARCH_SORT,
  THRESHOLDS,
  buildReport,
  errorText,
  findMemoryPeak,
  makeFailure,
  memoryDelta,
  memoryPeakDelta,
  normalizeSearchPage,
  parseArgs,
  parseMeminfoMetric,
  percentile,
  readAndroidMemory,
  reportChecks,
  roundMs,
  searchFilter,
  summarizeDurations,
  syntheticCorpusMissing,
  validateCorpusStatus,
  validateQuerySemantics,
  usage,
  unavailableReading,
  validateIndexStatus,
};
