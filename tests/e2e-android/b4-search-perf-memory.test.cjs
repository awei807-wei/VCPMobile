"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  isRssReading,
  memoryDelta,
  memoryPeakDelta,
  parseMeminfoMetric,
  parseProcRss,
  readAndroidMemory,
} = require("./b4-search-perf-memory.cjs");

test("RSS 严格采样优先通过 expected PID 的同 UID /proc 读取", () => {
  const calls = [];
  const reading = readAndroidMemory({
    requireRss: true,
    pid: 4_321,
    runAdb: (args) => {
      calls.push(args);
      if (args.at(-1) === "/proc/4321/status") return "VmRSS: 128 kB\n";
      throw new Error(`不应读取 ${args.join(" ")}`);
    },
  });
  assert.equal(reading.available, true);
  assert.equal(reading.rssAvailable, true);
  assert.equal(reading.metric, "rss");
  assert.equal(reading.pid, 4321);
  assert.equal(reading.bytes, 128 * 1024);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].slice(0, 4), [
    "shell",
    "run-as",
    "com.vcp.avatar.debug",
    "cat",
  ]);
});

test("/proc 不可用时可读取 PID-specific RSS meminfo，但拒绝 PSS 伪装成 RSS", () => {
  const rss = readAndroidMemory({
    requireRss: true,
    pid: 4_321,
    runAdb: (args) => {
      if (args[1] === "dumpsys" && args.at(-1) === "4321") {
        return "TOTAL RSS: 256 kB\nTOTAL PSS: 512 kB\n";
      }
      throw new Error("proc denied");
    },
  });
  assert.equal(rss.available, true);
  assert.equal(rss.rssAvailable, true);
  assert.equal(rss.metric, "rss");
  assert.equal(rss.pid, 4_321);
  assert.equal(rss.bytes, 256 * 1024);

  const pssOnly = parseMeminfoMetric("TOTAL PSS: 512 kB\n", {
    requireRss: true,
  });
  assert.equal(pssOnly.available, false);
  assert.equal(pssOnly.rssAvailable, false);
  assert.equal(pssOnly.metric, null);
});

test("严格 RSS 缺少 expected PID 时拒绝 pidof 和 package fallback", () => {
  const calls = [];
  const reading = readAndroidMemory({
    requireRss: true,
    runAdb: (args) => {
      calls.push(args);
      return "4321\nTOTAL RSS: 128 kB\n";
    },
  });
  assert.equal(reading.rssAvailable, false);
  assert.match(reading.note, /拒绝 pidof/);
  assert.equal(calls.length, 0);
});

test("PSS 或缺失样本会使内存门禁 fail-closed", () => {
  const pss = { available: true, rssAvailable: false, metric: "pss", bytes: 100 };
  const rss = { available: true, rssAvailable: true, metric: "rss", bytes: 110 };
  assert.equal(memoryDelta(rss, pss, 200).gatePassed, false);
  assert.equal(memoryPeakDelta(rss, pss, [rss, pss], 200).gatePassed, false);
  assert.equal(memoryPeakDelta(rss, null, [rss], 200).peakSampled, false);
});

function validRss(phase, bytes) {
  return {
    phase,
    available: true,
    rssAvailable: true,
    metric: "rss",
    bytes,
    pid: 7_001,
  };
}

test("RSS 读数的可用标志、RSS 标志、字节数和 PID 均严格校验", () => {
  const before = validRss("before", 100);
  const after = validRss("after", 110);
  const peak = validRss("during", 120);
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
    ["bytes 为负数", (reading) => (reading.bytes = -1)],
    ["bytes 非数字", (reading) => (reading.bytes = "100")],
    ["PID 缺失", (reading) => delete reading.pid],
    ["PID 非正整数", (reading) => (reading.pid = 0)],
  ];
  for (const [label, mutate] of invalidCases) {
    const invalid = validRss("after", 110);
    mutate(invalid);
    assert.equal(isRssReading(invalid), false, label);
    assert.equal(
      memoryDelta(before, invalid, 200, { requireSamePid: true }).gatePassed,
      false,
      label,
    );
    const peakResult = memoryPeakDelta(
      before,
      peak,
      [before, { ...invalid, phase: "during" }, after],
      200,
      { expectedPid: 7_001 },
    );
    assert.equal(peakResult.peakSampled, false, label);
    assert.equal(peakResult.gatePassed, false, label);
  }
});

test("RSS 最大安全整数边界在 delta 与 peak 门禁中仍合法", () => {
  const before = validRss("before", Number.MAX_SAFE_INTEGER - 100);
  const during = validRss("during", Number.MAX_SAFE_INTEGER);
  const after = validRss("after", Number.MAX_SAFE_INTEGER);
  assert.equal(isRssReading(during), true);
  assert.equal(
    memoryDelta(before, after, 200, { requireSamePid: true }).gatePassed,
    true,
  );
  const peak = memoryPeakDelta(
    before,
    during,
    [before, during, after],
    200,
    { expectedPid: 7_001 },
  );
  assert.equal(peak.peakSampled, true);
  assert.equal(peak.gatePassed, true);
});

test("重建窗口 baseline、during、after、peak PID 不一致时拒绝门禁", () => {
  const baseline = {
    phase: "before",
    available: true,
    rssAvailable: true,
    metric: "rss",
    bytes: 100,
    pid: 7_001,
  };
  const during = { ...baseline, phase: "during", bytes: 120, pid: 7_002 };
  const after = { ...baseline, phase: "after", bytes: 110 };
  const peak = { ...during };
  const delta = memoryDelta(baseline, after, 200, { requireSamePid: true });
  const peakDelta = memoryPeakDelta(baseline, peak, [baseline, during, after], 200);
  assert.equal(delta.gatePassed, true);
  assert.equal(peakDelta.gatePassed, false);
  assert.equal(peakDelta.pidConsistent, false);
  assert.match(peakDelta.reason, /PID 缺失或不一致/);
});

test("before/after 有效但没有 during 时峰值门禁明确失败", () => {
  const before = {
    phase: "before",
    available: true,
    rssAvailable: true,
    metric: "rss",
    bytes: 100,
    pid: 7_001,
  };
  const after = { ...before, phase: "after", bytes: 110 };
  const result = memoryPeakDelta(before, after, [before, after], 200, {
    expectedPid: 7_001,
  });
  assert.equal(result.gatePassed, false);
  assert.equal(result.peakSampled, false);
  assert.match(result.reason, /during/);
});

test("缺少 before、after 或任一样本无效时峰值门禁均拒绝", () => {
  const before = {
    phase: "before",
    available: true,
    rssAvailable: true,
    metric: "rss",
    bytes: 100,
    pid: 7_001,
  };
  const during = { ...before, phase: "during", bytes: 120 };
  const after = { ...before, phase: "after", bytes: 110 };
  const cases = [
    [null, after, [during, after]],
    [before, null, [before, during]],
    [before, after, [before, { ...during, bytes: null }, after]],
  ];
  for (const [baseline, final, samples] of cases) {
    const result = memoryPeakDelta(baseline, final, samples, 200, {
      expectedPid: 7_001,
    });
    assert.equal(result.gatePassed, false);
    assert.equal(result.peakSampled, false);
  }
});

test("RSS 字段解析支持 /proc 的单位并保留首个有效来源", () => {
  const reading = parseProcRss("VmRSS: 2 MB\nRss: 7 KB\n");
  assert.equal(reading.metric, "rss");
  assert.equal(reading.field, "VmRSS");
  assert.equal(reading.bytes, 2 * 1024 * 1024);
});
