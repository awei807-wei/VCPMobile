"use strict";

const { DEBUG_PACKAGE, runAdb } = require("./scripts/adb-env.cjs");

const PACKAGE_MEMORY_SOURCE = `adb shell dumpsys meminfo ${DEBUG_PACKAGE}`;
const PROC_STATUS_SOURCE = `adb shell run-as ${DEBUG_PACKAGE} cat /proc/<pid>/status`;
const PROC_SMAPS_SOURCE = `adb shell run-as ${DEBUG_PACKAGE} cat /proc/<pid>/smaps_rollup`;

function pidMemorySource(pid) {
  return `adb shell dumpsys meminfo ${pid}`;
}

function procMemorySource(pid, path) {
  return `adb shell run-as ${DEBUG_PACKAGE} cat /proc/${pid}/${path}`;
}

function parseUnitMultiplier(unit) {
  switch ((unit || "KB").toUpperCase()) {
    case "B":
      return 1;
    case "MB":
      return 1024 * 1024;
    case "GB":
      return 1024 * 1024 * 1024;
    case "KB":
    default:
      return 1024;
  }
}

function unavailableReading(note, extra = {}) {
  return {
    available: false,
    rssAvailable: false,
    metric: null,
    field: null,
    rawValue: null,
    rawUnit: null,
    bytes: null,
    source: null,
    note,
    ...extra,
  };
}

function makeReading(metric, field, amount, unit, source, extra = {}) {
  const rawValue = Number(String(amount).replace(/,/g, ""));
  const rawUnit = (unit || "KB").toUpperCase();
  const bytes = rawValue * parseUnitMultiplier(rawUnit);
  if (
    !Number.isSafeInteger(rawValue) ||
    rawValue < 0 ||
    !Number.isSafeInteger(bytes)
  ) {
    return null;
  }
  return {
    available: true,
    rssAvailable: metric === "rss",
    metric,
    field,
    rawValue,
    rawUnit,
    bytes,
    source,
    note: null,
    ...extra,
  };
}

function isPid(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function isRssReading(reading) {
  return (
    reading?.available === true &&
    reading?.rssAvailable === true &&
    reading?.metric === "rss" &&
    Number.isSafeInteger(reading?.bytes) &&
    reading.bytes >= 0 &&
    isPid(reading?.pid)
  );
}

/** Parse Android dumpsys meminfo, optionally refusing its PSS fallback. */
function parseMeminfoMetric(output, { requireRss = false, pid = null } = {}) {
  const text = typeof output === "string" ? output : "";
  const metrics = requireRss ? ["RSS"] : ["RSS", "PSS"];
  const source = isPid(pid) ? pidMemorySource(pid) : PACKAGE_MEMORY_SOURCE;
  for (const metric of metrics) {
    const pattern = new RegExp(
      `^\\s*TOTAL\\s+${metric}:\\s*([\\d,]+)\\s*([KMGT]?B)?\\b`,
      "im",
    );
    const match = text.match(pattern);
    if (!match) continue;
    const reading = makeReading(
      metric.toLowerCase(),
      `TOTAL ${metric}`,
      match[1],
      match[2],
      source,
      isPid(pid) ? { pid } : {},
    );
    if (reading) return reading;
  }
  const hasPss = new RegExp("^\\s*TOTAL\\s+PSS:", "im").test(text);
  return unavailableReading(
    requireRss
      ? hasPss
        ? "严格 RSS 模式检测到 TOTAL PSS，但 PSS 不可替代 RSS。"
        : "meminfo 未提供 TOTAL RSS。"
      : "meminfo 未提供 TOTAL RSS 或 TOTAL PSS。",
    {
      source,
      requestedMetric: requireRss ? "rss" : "rss-or-pss",
    },
  );
}

function parseProcRss(output, field = null, source = PROC_STATUS_SOURCE) {
  const text = typeof output === "string" ? output : "";
  const fields = field ? [field] : ["VmRSS", "Rss"];
  for (const candidate of fields) {
    const pattern = new RegExp(
      `^\\s*${candidate}:\\s*([\\d,]+)\\s*([KMGT]?B)?\\b`,
      "im",
    );
    const match = text.match(pattern);
    if (!match) continue;
    const reading = makeReading(
      "rss",
      candidate,
      match[1],
      match[2],
      source,
    );
    if (reading) return reading;
  }
  return null;
}

function parsePid(output) {
  const match = String(output || "").trim().match(/\b\d+\b/);
  if (!match) return null;
  const pid = Number(match[0]);
  return isPid(pid) ? pid : null;
}

function adbErrorText(error) {
  return error instanceof Error ? error.message : String(error);
}

function resolveRequestedPid(options) {
  const hasPid =
    Object.prototype.hasOwnProperty.call(options, "pid") ||
    Object.prototype.hasOwnProperty.call(options, "expectedPid");
  const aliasesAgree =
    options.pid === undefined ||
    options.expectedPid === undefined ||
    options.pid === options.expectedPid;
  const pid = options.pid ?? options.expectedPid ?? null;
  return { hasPid, pid: aliasesAgree && isPid(pid) ? pid : null };
}

function readProcMetric(adb, pid, path, field, source, errors) {
  try {
    const output = adb(
      ["shell", "run-as", DEBUG_PACKAGE, "cat", `/proc/${pid}/${path}`],
      { allowFailure: true, maxBuffer: 1024 * 1024 },
    );
    const reading = parseProcRss(output, field, source);
    return reading ? { ...reading, pid } : null;
  } catch (error) {
    errors.push(adbErrorText(error));
    return null;
  }
}

function readPinnedProcMemory(adb, pid, errors) {
  return (
    readProcMetric(
      adb,
      pid,
      "status",
      "VmRSS",
      procMemorySource(pid, "status"),
      errors,
    ) ||
    readProcMetric(
      adb,
      pid,
      "smaps_rollup",
      "Rss",
      procMemorySource(pid, "smaps_rollup"),
      errors,
    )
  );
}

function readPinnedMeminfo(adb, pid, requireRss, errors) {
  try {
    const output = adb(["shell", "dumpsys", "meminfo", String(pid)], {
      allowFailure: true,
      maxBuffer: 32 * 1024 * 1024,
    });
    return parseMeminfoMetric(output, { requireRss, pid });
  } catch (error) {
    errors.push(adbErrorText(error));
    return unavailableReading("PID-specific dumpsys meminfo 读取失败。", {
      source: pidMemorySource(pid),
      requestedMetric: requireRss ? "rss" : "rss-or-pss",
    });
  }
}

function pinnedMemoryReading(adb, pid, requireRss) {
  const errors = [];
  const procReading = readPinnedProcMemory(adb, pid, errors);
  if (procReading) return procReading;
  const reading = readPinnedMeminfo(adb, pid, requireRss, errors);
  if (reading.rssAvailable || (!requireRss && reading.available)) {
    return { ...reading, pid };
  }
  return unavailableReading(
    requireRss
      ? "严格 RSS 模式未取得 VmRSS、smaps_rollup Rss 或 PID-specific TOTAL RSS；TOTAL PSS 不可替代 RSS。"
      : reading.note,
    {
      source: [
        procMemorySource(pid, "status"),
        procMemorySource(pid, "smaps_rollup"),
        pidMemorySource(pid),
      ],
      requestedMetric: requireRss ? "rss" : "rss-or-pss",
      pid,
      expectedPid: pid,
      errors: errors.length > 0 ? errors.slice(0, 3) : [],
      pssAvailable: reading.metric === "pss" || reading.note?.includes("PSS"),
    },
  );
}

function readLegacyMemory(adb, requireRss) {
  const errors = [];
  let packagePid = null;
  try {
    packagePid = parsePid(
      adb(["shell", "pidof", DEBUG_PACKAGE], {
        allowFailure: true,
        maxBuffer: 1024 * 1024,
      }),
    );
  } catch (error) {
    errors.push(adbErrorText(error));
  }
  if (packagePid) return pinnedMemoryReading(adb, packagePid, requireRss);
  try {
    const output = adb(["shell", "dumpsys", "meminfo", DEBUG_PACKAGE], {
      allowFailure: true,
      maxBuffer: 32 * 1024 * 1024,
    });
    const reading = parseMeminfoMetric(output, { requireRss });
    if (reading.rssAvailable || (!requireRss && reading.available)) {
      return reading;
    }
    return unavailableReading(reading.note, {
      source: PACKAGE_MEMORY_SOURCE,
      requestedMetric: requireRss ? "rss" : "rss-or-pss",
      errors: errors.slice(0, 3),
    });
  } catch (error) {
    errors.push(adbErrorText(error));
    return unavailableReading("package 级 dumpsys meminfo 读取失败。", {
      source: PACKAGE_MEMORY_SOURCE,
      requestedMetric: requireRss ? "rss" : "rss-or-pss",
      errors: errors.slice(0, 3),
    });
  }
}

function readAndroidMemory(options = {}) {
  const { requireRss = false, runAdb: adb = runAdb } = options;
  const requested = resolveRequestedPid(options);
  if (requested.hasPid && !requested.pid) {
    return unavailableReading(
      "RSS 采样要求有效的 expected PID；拒绝不明确的 PID。",
      { requestedMetric: requireRss ? "rss" : "rss-or-pss" },
    );
  }
  if (requested.pid) return pinnedMemoryReading(adb, requested.pid, requireRss);
  if (requireRss) {
    return unavailableReading(
      "严格 RSS 模式缺少 expected PID；拒绝 pidof 和 package 级 fallback。",
      { requestedMetric: "rss" },
    );
  }
  return readLegacyMemory(adb, requireRss);
}

module.exports = {
  isPid,
  isRssReading,
  parseMeminfoMetric,
  parseProcRss,
  readAndroidMemory,
  unavailableReading,
};
