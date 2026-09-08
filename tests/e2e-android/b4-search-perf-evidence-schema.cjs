"use strict";

function redactText(value, queries) {
  if (typeof value !== "string") return value;
  const sensitiveQueries = [
    ...new Set(
      (Array.isArray(queries) ? queries : []).filter(
        (query) => typeof query === "string" && query.length > 0,
      ),
    ),
  ].sort((left, right) => right.length - left.length);
  const redactedQueries = sensitiveQueries.reduce(
    (text, query) => text.split(query).join("[已脱敏搜索词]"),
    value,
  );
  return redactedQueries
    .replace(/\bemulator-\d+\b/gi, "[已脱敏设备标识]")
    .replace(/(["']?serial["']?\s*[:=]\s*)(["'][^"']*["']|[^\s,;}]+)/gi, "$1[已脱敏设备标识]")
    .replace(/(["']?query["']?\s*[:=]\s*)(["'][^"']*["']|[^\s,;}]+)/gi, "$1[已脱敏搜索词]")
    .replace(/(ANDROID_SERIAL\s*[=:]\s*)[^\s,;}]+/gi, "$1[已脱敏设备标识]")
    .replace(/(^|\s)-s\s+[^\s]+/gi, "$1-s [已脱敏设备标识]")
    .replace(/\b[^\s"'`,;]*serial[^\s"'`,;]*\b/gi, "[已脱敏设备标识]")
    .replace(/(?:[A-Za-z]:[\\/]|\/)[^\s"'`,;]+/g, "[已脱敏路径]");
}
const ENUMS = Object.freeze({
  phase: new Set([
    "before",
    "during",
    "after",
    "warm",
    "cold",
    "warmup",
    "query",
    "query_semantics",
    "cold_restart",
    "rebuild",
    "memory_peak",
    "memory_sampling",
    "memory_before",
    "memory_after",
    "index_before",
    "index_after_rebuild",
    "corpus",
    "runtime",
    "cleanup",
    "timer",
  ]),
  metric: new Set(["rss", "pss"]),
  field: new Set(["VmRSS", "Rss", "TOTAL RSS", "TOTAL PSS"]),
  rawUnit: new Set(["B", "KB", "MB", "GB", "TB"]),
  requestedMetric: new Set(["rss", "rss-or-pss"]),
  category: new Set([
    "中文1字",
    "中文2字",
    "中文3+字",
    "英文",
    "特殊字符",
    "确定空结果",
    "自定义",
  ]),
  expectation: new Set(["non-empty", "empty"]),
  packageDebug: new Set(["com.vcp.avatar.debug"]),
  fixtureCommand: new Set([
    "prepare",
    "inject",
    "verify",
    "cleanup",
    "verify-device",
    "cleanup-device",
  ]),
  fixtureJournalMode: new Set(["delete"]),
  fixtureQuickCheck: new Set(["ok"]),
  fixtureCaseId: new Set([
    "default-zh-1",
    "default-zh-2",
    "default-zh-3",
    "default-en",
    "default-special",
    "default-empty",
  ]),
});
function enumRule(name) {
  return (value) => (ENUMS[name].has(value) ? value : undefined);
}
const RULES = {
  boolean: (value) => (typeof value === "boolean" ? value : undefined),
  integer: (value) =>
    Number.isSafeInteger(value) && value >= 0 ? value : undefined,
  signedInteger: (value) =>
    Number.isSafeInteger(value) ? value : undefined,
  number: (value) =>
    Number.isFinite(value) && value >= 0 ? value : undefined,
  pid: (value) =>
    Number.isSafeInteger(value) && value > 0 ? value : undefined,
  text: (value, queries) =>
    typeof value === "string" ? redactText(value, queries) : undefined,
  diagnostic: (value, queries) =>
    typeof value === "string" ? redactText(value, queries) : undefined,
  textOrTextArray: (value, queries) => {
    if (typeof value === "string") return redactText(value, queries);
    if (!Array.isArray(value)) return undefined;
    return value.every((item) => typeof item === "string")
      ? value.map((item) => redactText(item, queries))
      : undefined;
  },
  phase: enumRule("phase"),
  metric: enumRule("metric"),
  field: enumRule("field"),
  rawUnit: enumRule("rawUnit"),
  requestedMetric: enumRule("requestedMetric"),
  category: enumRule("category"),
  expectation: enumRule("expectation"),
  packageDebug: enumRule("packageDebug"),
  fixtureCommand: enumRule("fixtureCommand"),
  fixtureJournalMode: enumRule("fixtureJournalMode"),
  fixtureQuickCheck: enumRule("fixtureQuickCheck"),
  fixtureCaseId: enumRule("fixtureCaseId"),
};

const MEMORY_WINDOW_FIELDS = {
  metric: "optional:metric",
  rssAvailable: "optional:boolean",
  pidConsistent: "optional:boolean",
  expectedPid: "optional:pid",
  beforeBytes: "optional:integer",
  afterBytes: "optional:integer",
  deltaBytes: "optional:signedInteger",
  positiveDeltaBytes: "optional:integer",
  gatePassed: "optional:boolean",
  reason: "optional:diagnostic",
  peakSampled: "optional:boolean",
  baselinePid: "optional:pid",
  peakPid: "optional:pid",
  samplePids: "array:optional:pid",
  peakBytes: "optional:integer",
  peakDeltaBytes: "optional:signedInteger",
  positivePeakDeltaBytes: "optional:integer",
  before: "optional:dto:reading",
  after: "optional:dto:reading",
  peak: "optional:dto:reading",
  samples: "array:dto:reading",
  sampleCount: "optional:integer",
};

const DTO_FIELDS = {
  device: {
    manufacturer: "optional:text",
    model: "optional:text",
    sdk: "optional:integer",
    release: "optional:text",
    abi: "optional:text",
    packageDebug: "optional:packageDebug",
  },
  index: {
    available: "optional:boolean",
    schemaValid: "optional:boolean",
    tokenizerValid: "optional:boolean",
    topicCount: "optional:integer",
    liveTopicRowCount: "optional:integer",
    liveCount: "optional:integer",
    indexedCount: "optional:integer",
    missingCount: "optional:integer",
    orphanCount: "optional:integer",
    duplicateCount: "optional:integer",
    staleCount: "optional:integer",
    decodeErrorCount: "optional:integer",
    decodedContentBytes: "optional:integer",
    healthy: "optional:boolean",
    diagnostic: "optional:diagnostic",
  },
  corpusChecks: {
    statusAvailable: "optional:boolean",
    topicsExact: "optional:boolean",
    liveTopicRowsExact: "optional:boolean",
    messagesExact: "optional:boolean",
    decodedContentMinimum: "optional:boolean",
  },
  corpusRequirements: {
    topics: "optional:integer",
    liveTopicRows: "optional:integer",
    messages: "optional:integer",
    minDecodedContentBytes: "optional:integer",
  },
  corpus: {
    valid: "optional:boolean",
    checks: "optional:dto:corpusChecks",
    topicCount: "optional:integer",
    liveTopicRowCount: "optional:integer",
    messageCount: "optional:integer",
    decodedContentBytes: "optional:integer",
    requirements: "optional:dto:corpusRequirements",
    reason: "optional:diagnostic",
  },
  stats: {
    sampleCount: "optional:integer",
    minMs: "optional:number",
    medianMs: "optional:number",
    p95Ms: "optional:number",
    maxMs: "optional:number",
    meanMs: "optional:number",
  },
  restartGates: {
    processRestarted: "optional:boolean",
    foregrounded: "optional:boolean",
    previousPidChanged: "optional:boolean",
    nextPidMatchesCdpPid: "optional:boolean",
  },
  sample: {
    index: "optional:integer",
    phase: "optional:phase",
    durationMs: "optional:number",
    resultCount: "optional:integer",
    nextCursorPresent: "optional:boolean",
    restartMs: "optional:number",
    processRestarted: "optional:boolean",
    foregrounded: "optional:boolean",
    previousPid: "optional:pid",
    nextPid: "optional:pid",
    cdpPid: "optional:pid",
    restartGates: "optional:dto:restartGates",
    ok: "optional:boolean",
    errorCode: "optional:diagnostic",
    error: "optional:diagnostic",
  },
  validationCase: {
    observedCount: "optional:integer",
    ok: "optional:boolean",
    error: "optional:diagnostic",
  },
  validation: {
    applicable: "optional:boolean",
    valid: "optional:boolean",
    cases: "array:dto:validationCase",
    failures: "array:diagnostic",
  },
  failure: {
    phase: "optional:phase",
    message: "optional:diagnostic",
  },
  reading: {
    available: "optional:boolean",
    rssAvailable: "optional:boolean",
    metric: "optional:metric",
    field: "optional:field",
    rawValue: "optional:integer",
    rawUnit: "optional:rawUnit",
    bytes: "optional:integer",
    source: "optional:textOrTextArray",
    note: "optional:diagnostic",
    requestedMetric: "optional:requestedMetric",
    pid: "optional:pid",
    expectedPid: "optional:pid",
    errors: "array:diagnostic",
    pssAvailable: "optional:boolean",
    phase: "optional:phase",
    elapsedMs: "optional:number",
    error: "optional:diagnostic",
  },
  memoryFullWindow: {
    before: "optional:dto:reading",
    after: "optional:dto:reading",
    note: "optional:diagnostic",
  },
  memoryWindow: MEMORY_WINDOW_FIELDS,
  memory: {
    ...MEMORY_WINDOW_FIELDS,
    source: "optional:textOrTextArray",
    sources: "array:textOrTextArray",
    note: "optional:diagnostic",
    fullWindow: "optional:dto:memoryFullWindow",
    rebuildWindow: "optional:dto:memoryWindow",
  },
  rebuild: {
    wallClockMs: "optional:number",
    reportedDurationMs: "optional:number",
    indexedCount: "optional:integer",
    expectedPid: "optional:pid",
    memorySamples: "array:dto:reading",
    memoryPeak: "optional:dto:reading",
    memorySampleCount: "optional:integer",
    memorySamplingFailures: "array:dto:failure",
    ok: "optional:boolean",
    error: "optional:diagnostic",
  },
  fixtureRequirements: {
    topics: "optional:integer",
    liveTopicRows: "optional:integer",
    messages: "optional:integer",
    minDecodedContentBytes: "optional:integer",
  },
  fixtureChecks: {
    pageSize: "optional:boolean",
    journalMode: "optional:boolean",
    quickCheck: "optional:boolean",
    schema: "optional:boolean",
    tokenizer: "optional:boolean",
    topicsExact: "optional:boolean",
    liveTopicRowsExact: "optional:boolean",
    liveTopicRows: "optional:boolean",
    messagesExact: "optional:boolean",
    ftsExact: "optional:boolean",
    decodedContentMinimum: "optional:boolean",
    decodeErrorsZero: "optional:boolean",
    messagesContentText: "optional:boolean",
    ftsContentText: "optional:boolean",
    contentTypesText: "optional:boolean",
    semanticQueries: "optional:boolean",
    topicMessageCounts: "optional:boolean",
    deletedRowsExcluded: "optional:boolean",
    ownerTupleIsolation: "optional:boolean",
    sidecarsClean: "optional:boolean",
  },
  fixtureSemanticCase: {
    caseId: "optional:fixtureCaseId",
    category: "optional:category",
    expectation: "optional:expectation",
    nonEmpty: "optional:boolean",
    passed: "optional:boolean",
  },
  fixture: {
    command: "optional:fixtureCommand",
    ok: "optional:boolean",
    verified: "optional:boolean",
    prepared: "optional:boolean",
    injected: "optional:boolean",
    cleaned: "optional:boolean",
    serialTargetVerified: "optional:boolean",
    avdVerified: "optional:boolean",
    deviceFixtureVerified: "optional:boolean",
    remoteDatabaseVerified: "optional:boolean",
    runtimeStatusVerified: "optional:boolean",
    stagedFixtureVerified: "optional:boolean",
    packageDebug: "optional:packageDebug",
    pageSize: "optional:integer",
    journalMode: "optional:fixtureJournalMode",
    quickCheck: "optional:fixtureQuickCheck",
    schemaValid: "optional:boolean",
    tokenizerValid: "optional:boolean",
    topicCount: "optional:integer",
    liveTopicRowCount: "optional:integer",
    messageCount: "optional:integer",
    liveCount: "optional:integer",
    indexedCount: "optional:integer",
    decodedContentBytes: "optional:integer",
    decodeErrorCount: "optional:integer",
    semanticValid: "optional:boolean",
    requirements: "optional:dto:fixtureRequirements",
    checks: "optional:dto:fixtureChecks",
    semanticChecks: "array:dto:fixtureSemanticCase",
    residualCount: "optional:integer",
    error: "optional:diagnostic",
  },
};
function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function applyRule(value, rule, queries) {
  const optional = rule.startsWith("optional:");
  if (value === null) return optional ? null : undefined;
  const baseRule = optional ? rule.slice("optional:".length) : rule;
  if (baseRule.startsWith("array:")) {
    if (!Array.isArray(value)) return undefined;
    return value
      .map((item) => applyRule(item, baseRule.slice("array:".length), queries))
      .filter((item) => item !== undefined);
  }
  if (baseRule.startsWith("dto:")) {
    return sanitizeDto(value, baseRule.slice("dto:".length), queries);
  }
  return RULES[baseRule]?.(value, queries);
}

function sanitizeDto(value, name, queries) {
  if (value === null) return null;
  if (!isRecord(value) || !DTO_FIELDS[name]) return undefined;
  const safe = {};
  for (const [key, rule] of Object.entries(DTO_FIELDS[name])) {
    if (!Object.prototype.hasOwnProperty.call(value, key)) continue;
    const safeValue = applyRule(value[key], rule, queries);
    if (safeValue !== undefined) safe[key] = safeValue;
  }
  return safe;
}

function sanitizeSafeValue(value, queries, key = "") {
  if (DTO_FIELDS[key]) return sanitizeDto(value, key, queries);
  if (key === "phase") return applyRule(value, "optional:phase", queries);
  if (["error", "errorCode", "message", "note", "reason"].includes(key)) {
    return applyRule(value, "optional:diagnostic", queries);
  }
  return undefined;
}

module.exports = {
  isRecord,
  redactText,
  sanitizeDto,
  sanitizeSafeValue,
};
