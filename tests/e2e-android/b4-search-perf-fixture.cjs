"use strict";

const REQUIRED_CORPUS = Object.freeze({
  topics: 1_800,
  liveTopicRows: 1_800,
  messages: 50_000,
  minDecodedContentBytes: 100 * 1024 * 1024,
});

const DEFAULT_QUERY_SPECS = Object.freeze([
  {
    caseId: "default-zh-1",
    query: "全",
    category: "中文1字",
    expectation: "non-empty",
  },
  {
    caseId: "default-zh-2",
    query: "全局",
    category: "中文2字",
    expectation: "non-empty",
  },
  {
    caseId: "default-zh-3",
    query: "三个字",
    category: "中文3+字",
    expectation: "non-empty",
  },
  {
    caseId: "default-en",
    query: "English",
    category: "英文",
    expectation: "non-empty",
  },
  {
    caseId: "default-special",
    query: "a.b",
    category: "特殊字符",
    expectation: "non-empty",
  },
  {
    caseId: "default-empty",
    query: "b4-no-such-token-9f3e7c",
    category: "确定空结果",
    expectation: "empty",
  },
]);
const DEFAULT_SYNTHETIC_QUERIES = Object.freeze(
  DEFAULT_QUERY_SPECS.map((spec) => spec.query),
);

function readStatusValue(status, key, fallbackKey) {
  if (!status || typeof status !== "object") return undefined;
  return status[key] ?? (fallbackKey ? status[fallbackKey] : undefined);
}

function validateCorpusStatus(status) {
  const topicCount = readStatusValue(status, "topicCount", "topic_count");
  const messageCount = readStatusValue(status, "liveCount", "live_count");
  const liveTopicRowCount = readStatusValue(
    status,
    "liveTopicRowCount",
    "live_topic_row_count",
  );
  const decodedContentBytes = readStatusValue(
    status,
    "decodedContentBytes",
    "decoded_content_bytes",
  );
  const checks = {
    statusAvailable: status?.available === true,
    topicsExact:
      Number.isSafeInteger(topicCount) && topicCount === REQUIRED_CORPUS.topics,
    liveTopicRowsExact:
      Number.isSafeInteger(liveTopicRowCount) &&
      liveTopicRowCount === REQUIRED_CORPUS.liveTopicRows,
    messagesExact:
      Number.isSafeInteger(messageCount) &&
      messageCount === REQUIRED_CORPUS.messages,
    decodedContentMinimum:
      Number.isSafeInteger(decodedContentBytes) &&
      decodedContentBytes >= REQUIRED_CORPUS.minDecodedContentBytes,
  };
  const valid = Object.values(checks).every(Boolean);
  return {
    valid,
    checks,
    topicCount: Number.isSafeInteger(topicCount) ? topicCount : null,
    liveTopicRowCount: Number.isSafeInteger(liveTopicRowCount)
      ? liveTopicRowCount
      : null,
    messageCount: Number.isSafeInteger(messageCount) ? messageCount : null,
    decodedContentBytes: Number.isSafeInteger(decodedContentBytes)
      ? decodedContentBytes
      : null,
    requirements: REQUIRED_CORPUS,
    reason: valid
      ? null
      : "B4 语料门禁失败：需要 1800 个话题行、1800 个消息话题、50000 条消息及至少 100MiB 可解码正文",
  };
}

function validateQuerySemantics(warmups, querySource) {
  if (querySource !== "b4-synthetic-fixture") {
    return { applicable: false, valid: true, cases: [], failures: [] };
  }
  const cases = DEFAULT_QUERY_SPECS.map((spec) => {
    const sample = warmups.find((item) => item.query === spec.query);
    const observedCount = sample?.ok ? sample.resultCount : null;
    const valid =
      spec.expectation === "empty"
        ? observedCount === 0
        : Number.isSafeInteger(observedCount) && observedCount > 0;
    return {
      ...spec,
      observedCount,
      ok: valid,
      error: valid
        ? null
        : `${spec.category} 查询结果不符合预期（期望=${spec.expectation}，实际=${observedCount ?? "error"}）`,
    };
  });
  const failures = cases.filter((item) => !item.ok).map((item) => item.error);
  return { applicable: true, valid: failures.length === 0, cases, failures };
}

function syntheticCorpusMissing(warmups) {
  return validateQuerySemantics(warmups, "b4-synthetic-fixture")
    .cases.filter((item) => !item.ok)
    .map((item) => item.query);
}

function buildRuntimeFixtureValidation(state, indexStatus, packageName) {
  const corpus = state.corpusValidationBefore;
  const index = state.indexValidationBefore;
  const device = state.device;
  return {
    verified:
      packageName === "com.vcp.avatar.debug" &&
      device?.serialTargetVerified === true &&
      device?.avdVerified === true &&
      index?.healthy === true &&
      corpus?.valid === true,
    serialTargetVerified: device?.serialTargetVerified === true,
    avdVerified: device?.avdVerified === true,
    packageDebug: packageName,
    topicCount: Number.isSafeInteger(indexStatus?.topicCount)
      ? indexStatus.topicCount
      : null,
    liveTopicRowCount: Number.isSafeInteger(indexStatus?.liveTopicRowCount)
      ? indexStatus.liveTopicRowCount
      : Number.isSafeInteger(indexStatus?.live_topic_row_count)
        ? indexStatus.live_topic_row_count
        : null,
    messageCount: Number.isSafeInteger(indexStatus?.liveCount)
      ? indexStatus.liveCount
      : null,
    indexedCount: Number.isSafeInteger(indexStatus?.indexedCount)
      ? indexStatus.indexedCount
      : null,
    decodedContentBytes: Number.isSafeInteger(indexStatus?.decodedContentBytes)
      ? indexStatus.decodedContentBytes
      : null,
    requirements: REQUIRED_CORPUS,
  };
}

function maybeEnableSyntheticQueries(options, fixtureValidation) {
  if (
    options.querySource === "unverified-default" &&
    fixtureValidation.verified === true
  ) {
    options.querySource = "b4-synthetic-fixture";
  }
}

module.exports = {
  DEFAULT_QUERY_SPECS,
  DEFAULT_SYNTHETIC_QUERIES,
  REQUIRED_CORPUS,
  readStatusValue,
  syntheticCorpusMissing,
  validateCorpusStatus,
  validateQuerySemantics,
  buildRuntimeFixtureValidation,
  maybeEnableSyntheticQueries,
};
