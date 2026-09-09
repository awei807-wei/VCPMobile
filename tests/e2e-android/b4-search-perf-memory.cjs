"use strict";

const reading = require("./b4-search-perf-memory-reading.cjs");
const {
  isPid,
  isRssReading,
  parseMeminfoMetric,
  parseProcRss,
  readAndroidMemory,
  unavailableReading,
} = reading;

function samePid(before, after) {
  return isPid(before?.pid) && isPid(after?.pid) && before.pid === after.pid;
}

function resolveExpectedPid(before, options) {
  if (Object.prototype.hasOwnProperty.call(options, "expectedPid")) {
    return isPid(options.expectedPid) ? options.expectedPid : null;
  }
  return isPid(before?.pid) ? before.pid : null;
}

function pidMatches(reading, expectedPid) {
  return isPid(expectedPid) && reading?.pid === expectedPid;
}

function memoryDelta(before, after, limitBytes, options = {}) {
  const requireSamePid = options.requireSamePid === true;
  const hasExpected = Object.prototype.hasOwnProperty.call(options, "expectedPid");
  const expectedPid = resolveExpectedPid(before, options);
  const validRss = isRssReading(before) && isRssReading(after);
  const pidConsistent =
    (!requireSamePid || samePid(before, after)) &&
    (!hasExpected ||
      (pidMatches(before, expectedPid) && pidMatches(after, expectedPid)));
  if (!validRss || !pidConsistent) {
    return {
      metric: before?.metric || after?.metric || null,
      rssAvailable: false,
      pidConsistent,
      expectedPid,
      beforeBytes: before?.bytes ?? null,
      afterBytes: after?.bytes ?? null,
      deltaBytes: null,
      positiveDeltaBytes: null,
      gatePassed: false,
      reason:
        (requireSamePid || hasExpected) && !pidConsistent
          ? "重建窗口 RSS 采样 PID 缺失或不一致，内存门禁拒绝计算。"
          : "RSS 不可用，内存门禁拒绝使用 PSS 或缺失值。",
    };
  }
  const deltaBytes = after.bytes - before.bytes;
  return {
    metric: "rss",
    rssAvailable: true,
    pidConsistent,
    expectedPid,
    beforeBytes: before.bytes,
    afterBytes: after.bytes,
    deltaBytes,
    positiveDeltaBytes: Math.max(0, deltaBytes),
    gatePassed: deltaBytes <= limitBytes,
    reason: deltaBytes <= limitBytes ? null : `RSS 前后差值超过 ${limitBytes} 字节。`,
  };
}

function peakGateContext(before, peak, samples, options) {
  const sampleList = Array.isArray(samples) ? samples : [];
  const expectedPid = resolveExpectedPid(before, options);
  const beforeSamples = sampleList.filter(
    (sample) => sample?.phase === "before",
  );
  const beforeIncluded = beforeSamples.some(
    (sample) =>
      isRssReading(sample) &&
      sample.bytes === before?.bytes &&
      pidMatches(sample, expectedPid),
  );
  const baselineValid =
    isRssReading(before) && pidMatches(before, expectedPid) && beforeIncluded;
  const afterSamples = sampleList.filter((sample) => sample?.phase === "after");
  const duringSamples = sampleList.filter(
    (sample) => sample?.phase === "during" && isRssReading(sample),
  );
  const afterValid = afterSamples.some(
    (sample) => isRssReading(sample) && pidMatches(sample, expectedPid),
  );
  const phasesValid = sampleList.every((sample) =>
    ["before", "during", "after"].includes(sample?.phase),
  );
  const allSamplesValid =
    sampleList.length > 0 &&
    phasesValid &&
    sampleList.every(
      (sample) => isRssReading(sample) && pidMatches(sample, expectedPid),
    );
  const peakValid = isRssReading(peak) && pidMatches(peak, expectedPid);
  const pidConsistent = baselineValid && allSamplesValid && peakValid;
  return {
    sampleList,
    expectedPid,
    baselineValid,
    afterValid,
    duringSamples,
    allSamplesValid,
    peakValid,
    pidConsistent,
  };
}

function peakFailure(context, before, peak) {
  const {
    expectedPid,
    baselineValid,
    afterValid,
    duringSamples,
    allSamplesValid,
    peakValid,
    pidConsistent,
    sampleList,
  } = context;
  let reason = "RSS 峰值门禁拒绝计算。";
  if (!baselineValid) reason = "RSS before 样本无效或 PID 不匹配。";
  else if (!afterValid) reason = "RSS after 样本无效或缺失。";
  else if (duringSamples.length === 0) {
    reason = "重建期间没有有效 RSS during 样本；不得用 before/after 冒充。";
  } else if (!pidConsistent || !allSamplesValid || !peakValid) {
    reason = "重建窗口 RSS 采样 PID 缺失或不一致，峰值门禁拒绝计算。";
  }
  return {
    metric: peak?.metric || before?.metric || null,
    rssAvailable: false,
    peakSampled: false,
    pidConsistent,
    expectedPid,
    baselinePid: isPid(before?.pid) ? before.pid : null,
    peakPid: isPid(peak?.pid) ? peak.pid : null,
    samplePids: sampleList.map((sample) =>
      isPid(sample?.pid) ? sample.pid : null,
    ),
    peakBytes: peak?.bytes ?? null,
    peakDeltaBytes: null,
    positivePeakDeltaBytes: null,
    gatePassed: false,
    reason,
  };
}

function memoryPeakDelta(before, peak, samples, limitBytes, options = {}) {
  const context = peakGateContext(before, peak, samples, options);
  const { duringSamples, baselineValid, afterValid, allSamplesValid, peakValid } =
    context;
  const valid =
    baselineValid &&
    afterValid &&
    duringSamples.length > 0 &&
    allSamplesValid &&
    peakValid;
  if (!valid) return peakFailure(context, before, peak);
  const peakDeltaBytes = peak.bytes - before.bytes;
  return {
    metric: "rss",
    rssAvailable: true,
    peakSampled: true,
    pidConsistent: true,
    expectedPid: context.expectedPid,
    baselinePid: before.pid,
    peakPid: peak.pid,
    samplePids: context.sampleList.map((sample) => sample.pid),
    peakBytes: peak.bytes,
    peakDeltaBytes,
    positivePeakDeltaBytes: Math.max(0, peakDeltaBytes),
    gatePassed: peakDeltaBytes <= limitBytes,
    reason:
      peakDeltaBytes <= limitBytes
        ? null
        : `RSS 峰值差值超过 ${limitBytes} 字节。`,
  };
}

function findMemoryPeak(samples) {
  const valid = (samples || []).filter(isRssReading);
  if (valid.length === 0) return null;
  return valid.reduce((peak, sample) =>
    sample.bytes > peak.bytes ? sample : peak,
  );
}

module.exports = {
  findMemoryPeak,
  isPid,
  isRssReading,
  memoryDelta,
  memoryPeakDelta,
  parseMeminfoMetric,
  parseProcRss,
  readAndroidMemory,
  unavailableReading,
};
