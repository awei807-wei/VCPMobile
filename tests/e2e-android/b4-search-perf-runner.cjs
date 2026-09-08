"use strict";

const {
  DEBUG_PACKAGE,
  runAdb,
} = require("./scripts/adb-env.cjs");
const { reconnectAndroidCdp } = require("./wire14/android-state.cjs");
const {
  errorText,
  INDEX_STATUS_CDP_TIMEOUT_MS,
  makeFailure,
  normalizeSearchPage,
  REBUILD_CDP_TIMEOUT_MS,
  readAndroidMemory,
  roundMs,
  searchFilter,
  summarizeDurations,
  validateCorpusStatus,
  validateQuerySemantics,
  validateIndexStatus,
  unavailableReading,
} = require("./b4-search-perf-metrics.cjs");
const { ensureBenchmarkDevice } = require("./b4-search-perf-device.cjs");
const {
  buildRuntimeFixtureValidation,
  maybeEnableSyntheticQueries,
} = require("./b4-search-perf-fixture.cjs");
const {
  assertCdpBinding,
  MEMORY_SAMPLE_INTERVAL_MS,
  getVerifiedCdpPid,
  readStrictMemory,
  rebuildIndex,
} = require("./b4-search-perf-rebuild.cjs");
const {
  CORE_READY_POLL_INTERVAL_MS,
  CORE_READY_TIMEOUT_MS,
  CORE_STATUS_CDP_TIMEOUT_MS,
  normalizeCoreStatus,
  waitForCoreReady,
} = require("./b4-search-perf-readiness.cjs");
const {
  COLD_RESTART_POLL_INTERVAL_MS,
  COLD_RESTART_TIMEOUT_MS,
  restartB4ColdRuntime,
} = require("./b4-search-perf-restart.cjs");
const {
  coldRestartSampleChecks,
} = require("./b4-search-perf-gates.cjs");
const {
  recordIndexFailure,
  recordSearchFailures,
} = require("./b4-search-perf-runner-gates.cjs");

const EXPECTED_DEBUG_PACKAGE = "com.vcp.avatar.debug";

function initialState(device = null) {
  return {
    runtime: null,
    device,
    indexBefore: null,
    indexAfterRebuild: null,
    indexValidationBefore: null,
    indexValidationAfterRebuild: null,
    corpusValidationBefore: null,
    warm: null,
    cold: null,
    rebuild: null,
    memoryBefore: null,
    memoryAfter: null,
    memorySamples: [],
    memoryPeak: null,
    rebuildMemoryBefore: null,
    rebuildMemoryAfter: null,
    rebuildMemorySamples: [],
    rebuildMemoryPeak: null,
    fixtureValidation: null,
    failures: [],
  };
}

async function openRuntime(dependencies = {}) {
  const launch =
    dependencies.launch ||
    (() =>
      runAdb(["shell", "monkey", "-p", DEBUG_PACKAGE, "1"], {
        allowFailure: false,
      }));
  const reconnect = dependencies.reconnect || reconnectAndroidCdp;
  const waitReady = dependencies.waitReady || waitForCoreReady;
  await launch();
  const runtime = { cdp: await reconnect() };
  try {
    await waitReady(runtime.cdp.cdp);
    return runtime;
  } catch (error) {
    await runtime.cdp.close().catch(() => {});
    throw error;
  }
}

async function readIndexStatus(runtime) {
  return runtime.cdp.cdp.invoke(
    "get_fts_index_status",
    {},
    { timeoutMs: INDEX_STATUS_CDP_TIMEOUT_MS },
  );
}

function restartSampleFields(restartResult) {
  return {
    processRestarted: restartResult?.processRestarted ?? null,
    foregrounded: restartResult?.foregrounded ?? null,
    previousPid: restartResult?.previousPid ?? null,
    nextPid: restartResult?.nextPid ?? null,
    cdpPid: restartResult?.cdpPid ?? null,
  };
}

function resolveReadMemory(dependencies = {}) {
  return dependencies.readMemory || readAndroidMemory;
}
function readBoundMemory(runtime, readMemory, phase, failures) {
  let expectedPid = null;
  try {
    expectedPid = getVerifiedCdpPid(runtime);
    const binding = assertCdpBinding(runtime, expectedPid);
    const reading = readStrictMemory(readMemory, expectedPid);
    assertCdpBinding(runtime, expectedPid, binding);
    return { ...reading, phase, expectedPid };
  } catch (error) {
    const message = errorText(error);
    failures.push(makeFailure(phase, message));
    return unavailableReading(`RSS 采样失败：${message}`, {
      phase,
      pid: expectedPid,
      expectedPid,
      error: message,
    });
  }
}
async function timedSearch(cdp, query, index, phase, restartMs = null) {
  const started = performance.now();
  try {
    const page = await cdp.invoke("search_messages_fts", {
      filter: searchFilter(query),
    });
    const parsed = normalizeSearchPage(page);
    return {
      index,
      phase,
      query,
      durationMs: roundMs(performance.now() - started),
      resultCount: parsed.resultCount,
      nextCursorPresent: parsed.nextCursorPresent,
      restartMs,
      ok: true,
    };
  } catch (error) {
    return {
      index,
      phase,
      query,
      durationMs: roundMs(performance.now() - started),
      resultCount: null,
      nextCursorPresent: null,
      restartMs,
      ok: false,
      errorCode: error?.code || null,
      error: errorText(error),
    };
  }
}
async function warmSearch(cdp, queries, sampleCount, querySource) {
  const warmups = [];
  for (const query of queries) {
    const started = performance.now();
    try {
      const parsed = normalizeSearchPage(
        await cdp.invoke("search_messages_fts", { filter: searchFilter(query) }),
      );
      warmups.push({
        query,
        durationMs: roundMs(performance.now() - started),
        resultCount: parsed.resultCount,
        ok: true,
      });
    } catch (error) {
      warmups.push({
        query,
        durationMs: roundMs(performance.now() - started),
        resultCount: null,
        ok: false,
        error: errorText(error),
      });
    }
  }
  const queryValidation = validateQuerySemantics(warmups, querySource);
  if (!queryValidation.valid) {
    return {
      warmups,
      samples: [],
      stats: summarizeDurations([]),
      queryValidation,
    };
  }
  const samples = [];
  for (let index = 0; index < sampleCount; index += 1) {
    samples.push(
      await timedSearch(cdp, queries[index % queries.length], index + 1, "warm"),
    );
  }
  return { warmups, samples, stats: summarizeDurations(samples), queryValidation };
}
async function coldSearch(runtime, queries, sampleCount, dependencies = {}) {
  const restart = dependencies.restart || restartB4ColdRuntime;
  const waitReady = dependencies.waitReady || waitForCoreReady;
  const search = dependencies.timedSearch || timedSearch;
  const now = dependencies.now || (() => performance.now());
  const samples = [];
  for (let index = 0; index < sampleCount; index += 1) {
    const query = queries[index % queries.length];
    const restartStarted = now();
    let restartResult = null;
    try {
      restartResult = await restart(runtime);
      await waitReady(runtime.cdp.cdp);
      const sample = await search(
        runtime.cdp.cdp,
        query,
        index + 1,
        "cold",
        roundMs(now() - restartStarted),
      );
      Object.assign(sample, restartSampleFields(restartResult));
      sample.restartGates = coldRestartSampleChecks(sample);
      samples.push(sample);
    } catch (error) {
      samples.push({
        index: index + 1,
        phase: "cold",
        query,
        durationMs: null,
        resultCount: null,
        nextCursorPresent: null,
        restartMs: roundMs(now() - restartStarted),
        ...restartSampleFields(restartResult),
        restartGates: coldRestartSampleChecks(restartSampleFields(restartResult)),
        ok: false,
        errorCode: error?.code || null,
        error: errorText(error),
      });
      break;
    }
  }
  return { samples, stats: summarizeDurations(samples) };
}
async function runSearchPhases(state, options) {
  state.warm = await warmSearch(
    state.runtime.cdp.cdp,
    options.queries,
    options.samples,
    options.querySource,
  );
  state.cold = await coldSearch(
    state.runtime,
    options.queries,
    options.samples,
  );
  recordSearchFailures(state, options);
}
async function runRebuildAndIndex(state, dependencies = {}) {
  const expectedPid = getVerifiedCdpPid(state.runtime);
  state.rebuild = await rebuildIndex(state.runtime, dependencies);
  if (state.rebuild.expectedPid !== expectedPid) {
    state.failures.push(makeFailure("rebuild", "重建期间 CDP binding PID 已漂移"));
  }
  state.rebuildMemorySamples = state.rebuild.memorySamples || [];
  state.rebuildMemoryBefore =
    state.rebuildMemorySamples.find((sample) => sample.phase === "before") ||
    null;
  state.rebuildMemoryAfter =
    state.rebuildMemorySamples.find((sample) => sample.phase === "after") ||
    null;
  state.rebuildMemoryPeak = state.rebuild.memoryPeak || null;
  state.memorySamples = state.rebuildMemorySamples;
  state.memoryPeak = state.rebuildMemoryPeak;
  for (const failure of state.rebuild.memorySamplingFailures || []) {
    state.failures.push(makeFailure("memory_sampling", failure.message));
  }
  if (!state.rebuild.ok) state.failures.push(makeFailure("rebuild", "索引重建失败"));
  state.indexAfterRebuild = await readIndexStatus(state.runtime);
  state.indexValidationAfterRebuild = validateIndexStatus(state.indexAfterRebuild);
  recordIndexFailure(state, "index_after_rebuild", state.indexValidationAfterRebuild);
}
async function runMeasurementWindow(state, options, dependencies = {}) {
  const readMemory = resolveReadMemory(dependencies);
  state.memoryBefore = readBoundMemory(
    state.runtime,
    readMemory,
    "memory_before",
    state.failures,
  );
  try {
    await runSearchPhases(state, options);
    if (!state.runtime.cdp?.cdp) {
      state.failures.push(makeFailure("rebuild", "冷进程重连失败，无法继续重建测量"));
      return;
    }
    await runRebuildAndIndex(state, { ...dependencies, readMemory });
  } finally {
    state.memoryAfter = readBoundMemory(
      state.runtime,
      readMemory,
      "memory_after",
      state.failures,
    );
  }
}
async function closeRuntime(state) {
  const runtime = state.runtime?.cdp;
  if (!runtime || typeof runtime.close !== "function") return;
  try {
    await runtime.close();
  } catch (error) {
    state.failures.push(makeFailure("cleanup", errorText(error)));
  }
}

async function runBenchmark(options) {
  if (DEBUG_PACKAGE !== EXPECTED_DEBUG_PACKAGE) {
    throw new Error(`adb-env debug 包边界异常: ${DEBUG_PACKAGE}`);
  }
  const state = initialState(ensureBenchmarkDevice(options.serial));
  try {
    state.runtime = await openRuntime();
    state.indexBefore = await readIndexStatus(state.runtime);
    state.indexValidationBefore = validateIndexStatus(state.indexBefore);
    recordIndexFailure(state, "index_before", state.indexValidationBefore);
    state.corpusValidationBefore = validateCorpusStatus(state.indexBefore);
    state.fixtureValidation = buildRuntimeFixtureValidation(
      state,
      state.indexBefore,
      EXPECTED_DEBUG_PACKAGE,
    );
    maybeEnableSyntheticQueries(options, state.fixtureValidation);
    if (!state.corpusValidationBefore.valid) {
      state.failures.push(
        makeFailure("corpus", state.corpusValidationBefore.reason),
      );
    }
    if (!state.indexValidationBefore.healthy || !state.corpusValidationBefore.valid) {
      return state;
    }
    await runMeasurementWindow(state, options);
  } catch (error) {
    state.failures.push(makeFailure("runtime", errorText(error)));
  } finally {
    await closeRuntime(state);
  }
  return state;
}

module.exports = {
  COLD_RESTART_POLL_INTERVAL_MS,
  COLD_RESTART_TIMEOUT_MS,
  CORE_READY_POLL_INTERVAL_MS,
  CORE_READY_TIMEOUT_MS,
  CORE_STATUS_CDP_TIMEOUT_MS,
  MEMORY_SAMPLE_INTERVAL_MS,
  EXPECTED_DEBUG_PACKAGE,
  assertCdpBinding,
  getVerifiedCdpPid,
  INDEX_STATUS_CDP_TIMEOUT_MS,
  REBUILD_CDP_TIMEOUT_MS,
  coldSearch,
  initialState,
  normalizeCoreStatus,
  openRuntime,
  readIndexStatus,
  readBoundMemory,
  resolveReadMemory,
  rebuildIndex,
  restartB4ColdRuntime,
  runBenchmark,
  runMeasurementWindow,
  runRebuildAndIndex,
  waitForCoreReady,
};
