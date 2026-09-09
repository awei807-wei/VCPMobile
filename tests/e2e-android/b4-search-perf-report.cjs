"use strict";

const { CORE_READY_POLL_INTERVAL_MS, CORE_READY_TIMEOUT_MS } = require("./b4-search-perf-readiness.cjs");
const { COLD_RESTART_POLL_INTERVAL_MS, COLD_RESTART_TIMEOUT_MS } = require("./b4-search-perf-restart.cjs");
const { REQUIRED_CORPUS } = require("./b4-search-perf-fixture.cjs");
const { defaultQueryExpectations, queryCases, sanitizeSafeValue } = require("./b4-search-perf-evidence.cjs");

function buildReportConfiguration(options, state, constants) {
  return {
    samplesPerPhase: options.samples,
    queries: queryCases(options.queries, options.querySource),
    querySource: options.querySource,
    corpus:
      options.querySource === "b4-synthetic-fixture"
        ? "B4 合成库（预置消息）"
        : "未验证运行时语料",
    corpusRequirements: REQUIRED_CORPUS,
    corpusFacts: sanitizeSafeValue(
      state.fixtureValidation || state.corpusValidationBefore || null,
      options.queries,
      "corpus",
    ),
    fixture: sanitizeSafeValue(
      state.fixtureValidation || null,
      options.queries,
      "fixture",
    ),
    querySemanticExpectations: defaultQueryExpectations(),
    coldRestartStrategy: constants.coldRestartStrategy,
    coldRestartStrategyReason: constants.coldRestartStrategyReason,
    coldRestartStrategyBoundary: constants.coldRestartStrategyBoundary,
    coldRestartTimeoutMs: COLD_RESTART_TIMEOUT_MS,
    coldRestartPollIntervalMs: COLD_RESTART_POLL_INTERVAL_MS,
    coreReadyTimeoutMs: CORE_READY_TIMEOUT_MS,
    coreReadyPollIntervalMs: CORE_READY_POLL_INTERVAL_MS,
    searchFilter: { limit: constants.searchLimit, sort: constants.searchSort },
    percentile: "nearest-rank ceil(0.95 * n)",
    rebuildCdpTimeoutMs: constants.rebuildCdpTimeoutMs,
    indexStatusCdpTimeoutMs: constants.indexStatusCdpTimeoutMs,
  };
}

function uniqueReadingSources(state) {
  return [
    state.rebuildMemoryBefore,
    state.rebuildMemoryPeak,
    state.rebuildMemoryAfter,
  ]
    .map((reading) => reading?.source)
    .filter(Boolean)
    .filter((source, index, values) => values.indexOf(source) === index);
}

function buildMemoryReport(state, rebuildMemory, memoryPeak, expectedPid) {
  const samples = state.rebuildMemorySamples || [];
  const source = expectedPid
    ? `PID-bound RSS (/proc/${expectedPid} or dumpsys meminfo ${expectedPid})`
    : null;
  return {
    ...rebuildMemory,
    ...memoryPeak,
    before: state.rebuildMemoryBefore,
    after: state.rebuildMemoryAfter,
    peak: state.rebuildMemoryPeak,
    samples,
    sampleCount: samples.length,
    source,
    sources: uniqueReadingSources(state),
    note:
      rebuildMemory.rssAvailable && memoryPeak.rssAvailable
        ? null
        : "RSS 不可用或未完成峰值采样；PSS 不得作为 RSS 通过门禁。",
    fullWindow: {
      before: state.memoryBefore,
      after: state.memoryAfter,
      note: "全程观测跨越冷重启，仅记录读数，不计算跨 PID 差值且不参与重建门禁。",
    },
    rebuildWindow: {
      ...rebuildMemory,
      ...memoryPeak,
      before: state.rebuildMemoryBefore,
      after: state.rebuildMemoryAfter,
      peak: state.rebuildMemoryPeak,
      samples,
      sampleCount: samples.length,
    },
  };
}

module.exports = { buildMemoryReport, buildReportConfiguration };
