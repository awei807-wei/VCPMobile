"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  defaultQueryExpectations,
  queryEvidence,
  sanitizeSample,
} = require("./b4-search-perf-evidence.cjs");
const { buildReport } = require("./b4-search-perf-metrics.cjs");
const { parseArgs } = require("./b4-search-perf-cli.cjs");

test("自定义低熵词只产生不含输入信息的序号 evidence", () => {
  const lowEntropy = "a";
  const evidence = sanitizeSample(
    {
      query: lowEntropy,
      queryLength: 1,
      queryBytes: 1,
      queryHash: "ca978112ca1bbdcafac231b39a23dc4d",
      digest: "ca978112ca1bbdcafacac231b39a23dc4d",
      durationMs: 2,
      error: lowEntropy,
    },
    "cli",
    0,
    [lowEntropy],
  );
  const serialized = JSON.stringify(evidence);
  assert.equal(serialized.includes('"a"'), false);
  assert.equal(serialized.includes("queryLength"), false);
  assert.equal(serialized.includes("queryBytes"), false);
  assert.equal(serialized.includes("queryHash"), false);
  assert.equal(serialized.includes("digest"), false);
  assert.equal(evidence.caseId, "custom-01");
  assert.equal(queryEvidence("b", "cli", 0).caseId, evidence.caseId);
});

test("默认 evidence 仅保留固定语义类别，不输出默认原词", () => {
  const serialized = JSON.stringify(defaultQueryExpectations());
  for (const query of [
    "全",
    "全局",
    "三个字",
    "English",
    "a.b",
    "b4-no-such-token-9f3e7c",
  ]) {
    assert.equal(serialized.includes(query), false);
  }
  assert.equal(serialized.includes("queryLength"), false);
  assert.equal(serialized.includes("queryBytes"), false);
  assert.equal(serialized.includes("queryHash"), false);
});

test("buildReport 的 index/rebuild/memory/device 全树分支均丢弃未知嵌套载荷", () => {
  const secret = "b4-no-such-token-9f3e7c";
  const sensitive = {
    query: secret,
    queryLength: secret.length,
    queryBytes: Buffer.byteLength(secret, "utf8"),
    queryHash: "deadbeef",
    digest: "cafebabe",
    hash: "feedface",
    queryInfo: { length: secret.length, bytes: 99, unknown: secret },
    unknown: secret,
  };
  const withSensitive = (value) => ({
    ...value,
    nested: structuredClone(sensitive),
    unknownField: secret,
  });
  const reading = (phase, bytes) =>
    withSensitive({
      phase,
      available: true,
      rssAvailable: true,
      metric: "rss",
      bytes,
      pid: 7_001,
    });
  const report = buildReport(parseArgs([]), {
    device: withSensitive({ serial: "test-device" }),
    indexBefore: withSensitive({ healthy: true }),
    indexAfterRebuild: withSensitive({ healthy: true }),
    indexValidationBefore: { healthy: true },
    indexValidationAfterRebuild: { healthy: true },
    corpusValidationBefore: withSensitive({ valid: true }),
    warm: {
      warmups: [{ query: secret, ok: true, durationMs: 1 }],
      samples: [{ query: secret, ok: true, durationMs: 1 }],
      stats: { sampleCount: 1, p95Ms: 1 },
      queryValidation: { valid: true, cases: [], failures: [] },
    },
    cold: { samples: [{ query: secret, ok: true, durationMs: 1 }], stats: {} },
    rebuild: withSensitive({ ok: true, wallClockMs: 1, expectedPid: 7_001 }),
    memoryBefore: reading("before", 90),
    memoryAfter: reading("after", 110),
    rebuildMemoryBefore: reading("before", 100),
    rebuildMemoryAfter: reading("after", 101),
    rebuildMemorySamples: [
      reading("before", 100),
      reading("during", 102),
      reading("after", 101),
    ],
    rebuildMemoryPeak: reading("during", 102),
    failures: [],
  });
  const serialized = JSON.stringify(report);
  assert.equal(serialized.includes(secret), false);
  for (const field of [
    "query",
    "queryLength",
    "queryBytes",
    "queryHash",
    "digest",
    "hash",
  ]) {
    assert.equal(serialized.includes(`"${field}"`), false, field);
  }
  assert.equal(report.memory.rebuildWindow.before.bytes, 100);
  assert.equal(report.memory.rebuildWindow.after.bytes, 101);
  assert.equal(report.device.unknownField, undefined);
  assert.equal(report.device.nested, undefined);
  assert.equal(report.index.before.nested, undefined);
  assert.equal(report.rebuild.nested, undefined);
  assert.equal(report.memory.rebuildWindow.before.nested, undefined);
});

test("完整报告只保留字符串 query validation failure，并按查询上下文脱敏", () => {
  const secret = "query-validation-secret-9d4e";
  const marker = "object-only-diagnostic-marker-7c1a";
  const objectFailure = {
    query: secret,
    queryLength: secret.length,
    bytes: Buffer.byteLength(secret, "utf8"),
    hash: marker,
    digest: marker,
    nested: [{ query: secret, bytes: 99, hash: marker, digest: marker }],
  };
  const state = collisionState(secret);
  state.warm.queryValidation = {
    applicable: true,
    valid: true,
    cases: [],
    failures: [
      objectFailure,
      [objectFailure, { query: secret, queryLength: 1, bytes: 1 }],
      `查询校验失败：${secret}`,
    ],
  };
  const report = buildReport(
    parseArgs(["--query", secret]),
    state,
  );

  assert.deepEqual(report.warm.queryValidation.failures, [
    "查询校验失败：[已脱敏搜索词]",
  ]);
  const serialized = JSON.stringify(report);
  assert.equal(serialized.includes(secret), false);
  assert.equal(serialized.includes(marker), false);
});

function collisionReading(phase, bytes) {
  return {
    phase,
    available: true,
    rssAvailable: true,
    metric: "rss",
    bytes,
    pid: 7_001,
  };
}

function collisionState(query) {
  const before = collisionReading("before", 100);
  const during = collisionReading("during", 102);
  const after = collisionReading("after", 101);
  const sample = {
    index: 1,
    phase: "cold",
    query,
    durationMs: 3,
    resultCount: 2,
    nextCursorPresent: false,
    processRestarted: true,
    foregrounded: true,
    previousPid: 7_000,
    nextPid: 7_001,
    cdpPid: 7_001,
    ok: true,
  };
  return {
    device: {
      serial: "serial-1",
      model: "model-1",
      sdk: 35,
      packageDebug: "com.vcp.avatar.debug",
      unknownPayload: { query },
    },
    indexBefore: {
      available: true,
      schemaValid: true,
      tokenizerValid: true,
      topicCount: 1_800,
      liveCount: 50_000,
      indexedCount: 50_000,
      missingCount: 0,
      orphanCount: 0,
      duplicateCount: 0,
      staleCount: 0,
      decodeErrorCount: 0,
      decodedContentBytes: 100 * 1024 * 1024,
      healthy: true,
      unknownPayload: { query },
    },
    indexAfterRebuild: { healthy: true, available: true },
    indexValidationBefore: { healthy: true },
    indexValidationAfterRebuild: { healthy: true },
    corpusValidationBefore: {
      valid: true,
      checks: {
        statusAvailable: true,
        topicsExact: true,
        messagesExact: true,
        decodedContentMinimum: true,
      },
      topicCount: 1_800,
      messageCount: 50_000,
      decodedContentBytes: 100 * 1024 * 1024,
      requirements: {
        topics: 1_800,
        messages: 50_000,
        minDecodedContentBytes: 100 * 1024 * 1024,
      },
    },
    warm: {
      warmups: [{ query, ok: true, resultCount: 2, durationMs: 1 }],
      samples: [{ ...sample, phase: "warm", durationMs: 2 }],
      stats: {
        sampleCount: 1,
        minMs: 2,
        medianMs: 2,
        p95Ms: 2,
        maxMs: 2,
        meanMs: 2,
      },
      queryValidation: { applicable: false, valid: true, cases: [], failures: [] },
    },
    cold: { samples: [sample], stats: { sampleCount: 1, p95Ms: 3 } },
    rebuild: {
      ok: true,
      wallClockMs: 4,
      reportedDurationMs: 4,
      indexedCount: 50_000,
      expectedPid: 7_001,
      memorySamples: [before, during, after],
      memoryPeak: during,
      memorySampleCount: 3,
      memorySamplingFailures: [],
      unknownPayload: { query },
    },
    memoryBefore: before,
    memoryAfter: after,
    rebuildMemoryBefore: before,
    rebuildMemoryAfter: after,
    rebuildMemorySamples: [before, during, after],
    rebuildMemoryPeak: during,
    failures: [],
  };
}

test("低熵查询不改变完整报告的 schema 键、枚举和合法指标", () => {
  for (const query of ["a", "rss", "warm", "before"]) {
    const report = buildReport(
      { samples: 1, queries: [query], querySource: "cli" },
      collisionState(query),
    );
    assert.equal(report.ok, true, query);
    assert.equal(report.ok, Object.values(report.checks).every(Boolean), query);
    assert.equal(report.index.before.available, true, query);
    assert.equal(report.index.before.healthy, true, query);
    assert.equal(report.memory.rebuildWindow.before.phase, "before", query);
    assert.equal(report.memory.rebuildWindow.before.metric, "rss", query);
    assert.equal(report.memory.rebuildWindow.before.bytes, 100, query);
    assert.equal(report.warm.samples[0].phase, "warm", query);
    assert.equal(report.warm.samples[0].durationMs, 2, query);
    assert.equal(report.rebuild.memorySamples[1].phase, "during", query);
    assert.equal(report.configuration.queries[0].query, undefined, query);
    assert.equal(report.device.unknownPayload, undefined, query);
    assert.equal(report.rebuild.unknownPayload, undefined, query);
  }
});
