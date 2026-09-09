"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  buildReport,
  INDEX_STATUS_CDP_TIMEOUT_MS,
  REBUILD_CDP_TIMEOUT_MS,
  parseArgs,
} = require("./b4-search-perf-metrics.cjs");
const {
  coldRestartChecks,
  coldRestartGateFailures,
  coldRestartSampleChecks,
} = require("./b4-search-perf-gates.cjs");

const VALID_SAMPLE = Object.freeze({
  index: 1,
  phase: "cold",
  query: "needle",
  durationMs: 10,
  resultCount: 1,
  nextCursorPresent: false,
  restartMs: 100,
  processRestarted: true,
  foregrounded: true,
  previousPid: 7_000,
  nextPid: 7_001,
  cdpPid: 7_001,
  ok: true,
});

const REPORT_OPTIONS = {
  samples: 1,
  queries: ["needle"],
  querySource: "cli",
};

function sampleWith(changes = {}) {
  return { ...VALID_SAMPLE, ...changes };
}

function reportFor(sample, overrides = {}, options = REPORT_OPTIONS) {
  return buildReport(options, {
    device: null,
    indexBefore: { healthy: true },
    indexAfterRebuild: { healthy: true },
    indexValidationBefore: { healthy: true },
    indexValidationAfterRebuild: { healthy: true },
    corpusValidationBefore: { valid: true },
    warm: {
      warmups: [],
      samples: [{ ok: true, durationMs: 1 }],
      queryValidation: { valid: true },
      stats: {
        sampleCount: 1,
        minMs: 1,
        medianMs: 1,
        p95Ms: 1,
        maxMs: 1,
        meanMs: 1,
      },
    },
    cold: {
      samples: [sample],
      stats: {
        sampleCount: 1,
        minMs: 10,
        medianMs: 10,
        p95Ms: 10,
        maxMs: 10,
        meanMs: 10,
      },
    },
    rebuild: { ok: true, wallClockMs: 1 },
    memoryBefore: {
      available: true,
      rssAvailable: true,
      metric: "rss",
      bytes: 90,
      pid: 7_000,
    },
    memoryAfter: {
      available: true,
      rssAvailable: true,
      metric: "rss",
      bytes: 110,
      pid: 7_001,
    },
    rebuildMemoryBefore: {
      phase: "before",
      available: true,
      rssAvailable: true,
      metric: "rss",
      bytes: 100,
      pid: 7_001,
    },
    rebuildMemoryAfter: {
      phase: "after",
      available: true,
      rssAvailable: true,
      metric: "rss",
      bytes: 101,
      pid: 7_001,
    },
    rebuildMemorySamples: [
      {
        phase: "before",
        available: true,
        rssAvailable: true,
        metric: "rss",
        bytes: 100,
        pid: 7_001,
      },
      {
        phase: "during",
        available: true,
        rssAvailable: true,
        metric: "rss",
        bytes: 102,
        pid: 7_001,
      },
      {
        phase: "after",
        available: true,
        rssAvailable: true,
        metric: "rss",
        bytes: 101,
        pid: 7_001,
      },
    ],
    rebuildMemoryPeak: {
      phase: "during",
      available: true,
      rssAvailable: true,
      metric: "rss",
      bytes: 102,
      pid: 7_001,
    },
    failures: [],
    ...overrides,
  });
}

function assertFailureFor(sample, checkKey, messagePart) {
  const sampleChecks = coldRestartSampleChecks(sample);
  assert.equal(sampleChecks[checkKey], false);
  const failures = coldRestartGateFailures([sample]);
  assert.ok(
    failures.some((failure) => failure.message.includes(messagePart)),
    `${checkKey} 缺少失败记录`,
  );
}

test("四项冷重启 gate 对缺失字段逐项失败", () => {
  const missingCases = [
    ["processRestarted", "processRestarted", "processRestarted 必须为 true"],
    ["foregrounded", "foregrounded", "foregrounded 必须为 true"],
    ["previousPid", "previousPidChanged", "previousPid 必须与 nextPid 不同"],
    ["nextPid", "previousPidChanged", "previousPid 必须与 nextPid 不同"],
    ["cdpPid", "nextPidMatchesCdpPid", "nextPid 必须与 cdpPid 匹配"],
  ];
  for (const [field, checkKey, messagePart] of missingCases) {
    const sample = sampleWith();
    delete sample[field];
    assertFailureFor(sample, checkKey, messagePart);
  }
});

test("previousPid===nextPid、nextPid!==cdpPid、前台/重启标记异常均逐项失败", () => {
  assertFailureFor(
    sampleWith({ nextPid: 7_000 }),
    "previousPidChanged",
    "previousPid 必须与 nextPid 不同",
  );
  assertFailureFor(
    sampleWith({ cdpPid: 7_002 }),
    "nextPidMatchesCdpPid",
    "nextPid 必须与 cdpPid 匹配",
  );
  assertFailureFor(
    sampleWith({ foregrounded: false }),
    "foregrounded",
    "foregrounded 必须为 true",
  );
  assertFailureFor(
    sampleWith({ processRestarted: false }),
    "processRestarted",
    "processRestarted 必须为 true",
  );
});

test("正常样本总报告通过并暴露四项冷重启 checks", () => {
  const report = reportFor(sampleWith());
  assert.equal(report.ok, true);
  assert.equal(report.failures.length, 0);
  assert.equal(INDEX_STATUS_CDP_TIMEOUT_MS, 15_000);
  assert.equal(REBUILD_CDP_TIMEOUT_MS, 185_000);
  assert.notEqual(INDEX_STATUS_CDP_TIMEOUT_MS, REBUILD_CDP_TIMEOUT_MS);
  assert.equal(report.configuration.indexStatusCdpTimeoutMs, 15_000);
  assert.equal(report.configuration.rebuildCdpTimeoutMs, 185_000);
  assert.equal(report.checks.coldSamplesProcessRestarted, true);
  assert.equal(report.checks.coldSamplesForegrounded, true);
  assert.equal(report.checks.coldSamplesPreviousPidChanged, true);
  assert.equal(report.checks.coldSamplesNextPidMatchesCdpPid, true);
});

test("报告只输出查询 case evidence，不泄露默认或自定义原词", () => {
  const secret = "user-sensitive-search-7f3a";
  const options = parseArgs(["--query", secret]);
  const report = reportFor(
    sampleWith({ query: secret }),
    {
      warm: {
        warmups: [{ query: secret, ok: false, resultCount: null, error: secret }],
        samples: [{ query: secret, index: 1, ok: true, durationMs: 1 }],
        queryValidation: {
          valid: true,
          cases: [{ query: secret, observedCount: 1, ok: true }],
          failures: [],
        },
        stats: {
          sampleCount: 1,
          minMs: 1,
          medianMs: 1,
          p95Ms: 1,
          maxMs: 1,
          meanMs: 1,
        },
      },
      failures: [{ phase: "query", message: secret }],
    },
    options,
  );
  const serialized = JSON.stringify(report);
  assert.equal(serialized.includes(secret), false);
  assert.equal(report.configuration.queries[0].caseId, "custom-01");
  assert.equal(report.configuration.queries[0].query, undefined);
  assert.equal(report.warm.warmups[0].query, undefined);
  assert.equal(report.cold.samples[0].query, undefined);
  assert.equal(report.warm.warmups[0].queryLength, undefined);
  assert.equal(report.warm.warmups[0].queryBytes, undefined);
  assert.equal(report.warm.warmups[0].queryHash, undefined);
  assert.match(report.warm.warmups[0].error, /已脱敏搜索词/);

  const defaults = reportFor(sampleWith(), {}, parseArgs([]));
  const defaultJson = JSON.stringify(defaults);
  for (const spec of ["全", "全局", "三个字", "English", "a.b"]) {
    assert.equal(defaultJson.includes(`\"query\":\"${spec}\"`), false);
  }
  assert.deepEqual(
    defaults.configuration.querySemanticExpectations.map((item) => item.category),
    ["中文1字", "中文2字", "中文3+字", "英文", "特殊字符", "确定空结果"],
  );
});

test("每类坏样本都会让总报告失败并保留对应 gate 失败记录", () => {
  const badCases = [
    [
      "缺 previousPid",
      (() => {
        const sample = sampleWith();
        delete sample.previousPid;
        return sample;
      })(),
      "coldSamplesPreviousPidChanged",
      "previousPid 必须与 nextPid 不同",
    ],
    [
      "previousPid===nextPid",
      sampleWith({ nextPid: 7_000 }),
      "coldSamplesPreviousPidChanged",
      "previousPid 必须与 nextPid 不同",
    ],
    [
      "nextPid!==cdpPid",
      sampleWith({ cdpPid: 7_002 }),
      "coldSamplesNextPidMatchesCdpPid",
      "nextPid 必须与 cdpPid 匹配",
    ],
    [
      "foregrounded=false",
      sampleWith({ foregrounded: false }),
      "coldSamplesForegrounded",
      "foregrounded 必须为 true",
    ],
    [
      "processRestarted=false",
      sampleWith({ processRestarted: false }),
      "coldSamplesProcessRestarted",
      "processRestarted 必须为 true",
    ],
  ];
  for (const [label, sample, checkKey, messagePart] of badCases) {
    const report = reportFor(sample);
    assert.equal(report.ok, false, `${label} 未阻断总报告`);
    assert.equal(report.checks[checkKey], false, `${label} gate 未失败`);
    assert.ok(
      report.failures.some((failure) => failure.message.includes(messagePart)),
      `${label} 未保留对应失败记录`,
    );
  }
});

test("多样本中单个坏样本也不能被其他正常样本掩盖", () => {
  const report = buildReport(REPORT_OPTIONS, {
    ...reportFor(sampleWith()),
    cold: {
      samples: [sampleWith(), sampleWith({ index: 2, processRestarted: false })],
      stats: reportFor(sampleWith()).cold.stats,
    },
  });
  assert.equal(report.ok, false);
  assert.equal(report.checks.coldSamplesProcessRestarted, false);
  assert.ok(
    report.failures.some((failure) => failure.message.includes("样本 #1")) === false,
  );
  assert.ok(
    report.failures.some((failure) => failure.message.includes("样本 #2") && failure.message.includes("processRestarted")),
  );
});

test("空冷样本不会被 all/every 空集合误判为通过", () => {
  const checks = coldRestartChecks([]);
  assert.equal(checks.coldSamplesProcessRestarted, false);
  assert.equal(checks.coldSamplesForegrounded, false);
  assert.equal(checks.coldSamplesPreviousPidChanged, false);
  assert.equal(checks.coldSamplesNextPidMatchesCdpPid, false);
});

test("RSS 峰值超过门限会让总报告失败", () => {
  const report = reportFor(sampleWith(), {
    rebuildMemoryPeak: {
      phase: "during",
      available: true,
      rssAvailable: true,
      metric: "rss",
      bytes: 209715301,
      pid: 7_001,
    },
  });
  assert.equal(report.ok, false);
  assert.equal(report.checks.rssAvailable, true);
  assert.equal(report.checks.memoryPeakSampled, true);
  assert.equal(report.checks.memoryPeakWithinLimit, false);
  assert.equal(report.checks.memoryDeltaWithinLimit, true);
});

test("最终报告 checks 对所有矛盾 RSS 标志均 fail-closed", () => {
  const invalidCases = [
    ["available=false", (reading) => (reading.available = false)],
    ["available 缺失", (reading) => delete reading.available],
    ["available=undefined", (reading) => (reading.available = undefined)],
    ["rssAvailable=false", (reading) => (reading.rssAvailable = false)],
    ["rssAvailable 缺失", (reading) => delete reading.rssAvailable],
    ["rssAvailable=undefined", (reading) => (reading.rssAvailable = undefined)],
    ["metric 非 rss", (reading) => (reading.metric = "pss")],
    ["bytes 非有限", (reading) => (reading.bytes = Infinity)],
    ["bytes 为小数", (reading) => (reading.bytes = 0.5)],
    ["bytes 超出安全整数", (reading) => (reading.bytes = Number.MAX_SAFE_INTEGER + 1)],
    ["PID 缺失", (reading) => delete reading.pid],
  ];
  for (const [label, mutate] of invalidCases) {
    const invalid = {
      phase: "before",
      available: true,
      rssAvailable: true,
      metric: "rss",
      bytes: 100,
      pid: 7_001,
    };
    mutate(invalid);
    const report = reportFor(sampleWith(), { rebuildMemoryBefore: invalid });
    assert.equal(report.ok, false, label);
    for (const key of [
      "rssAvailable",
      "memoryRssAvailable",
      "memoryPeakSampled",
      "memoryPeakWithinLimit",
      "memoryDeltaWithinLimit",
    ]) {
      assert.equal(report.checks[key], false, `${label}: ${key}`);
    }
  }
});
