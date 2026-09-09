"use strict";

const { DEFAULT_QUERY_SPECS } = require("./b4-search-perf-fixture.cjs");
const {
  isRecord,
  redactText,
  sanitizeDto,
  sanitizeSafeValue,
} = require("./b4-search-perf-evidence-schema.cjs");

function querySpec(query, querySource) {
  if (querySource !== "b4-synthetic-fixture") return null;
  return DEFAULT_QUERY_SPECS.find((item) => item.query === query) || null;
}

function queryEvidence(query, querySource, index = 0) {
  const spec = querySpec(query, querySource);
  const ordinal = index + 1;
  const evidence = {
    caseId: spec?.caseId || `custom-${String(ordinal).padStart(2, "0")}`,
    category: spec?.category || "自定义",
    ordinal,
  };
  if (spec?.expectation) evidence.expectation = spec.expectation;
  return evidence;
}

function sanitizeSample(sample, querySource, index, queries) {
  const source = sample && typeof sample === "object" ? sample : {};
  const safe = sanitizeDto(source, "sample", queries) || {};
  const evidence = queryEvidence(source.query, querySource, index);
  return { ...safe, ...evidence };
}

function sanitizeQueryValidation(validation, querySource, queries) {
  if (!isRecord(validation)) return null;
  return {
    applicable: validation.applicable === true,
    valid: validation.valid === true,
    cases: Array.isArray(validation.cases)
      ? validation.cases.map((item, index) =>
          sanitizeValidationCase(item, querySource, index, queries),
        )
      : [],
    failures: Array.isArray(validation.failures)
      ? validation.failures
          .filter((failure) => typeof failure === "string")
          .map((failure) => redactText(failure, queries))
          .filter((failure) => failure !== undefined)
      : [],
  };
}

function sanitizeValidationCase(item, querySource, index, queries) {
  const source = item && typeof item === "object" ? item : {};
  const safe = sanitizeDto(source, "validationCase", queries) || {};
  return { ...safe, ...queryEvidence(source.query, querySource, index) };
}

function sanitizeStats(stats, queries) {
  return sanitizeDto(stats || {}, "stats", queries) || {};
}

function sanitizeWarm(warm, querySource, queries) {
  if (!isRecord(warm)) return null;
  return {
    warmups: Array.isArray(warm.warmups)
      ? warm.warmups.map((sample, index) =>
          sanitizeSample(sample, querySource, index, queries),
        )
      : [],
    samples: Array.isArray(warm.samples)
      ? warm.samples.map((sample, index) =>
          sanitizeSample(sample, querySource, index, queries),
        )
      : [],
    stats: sanitizeStats(warm.stats, queries),
    queryValidation: sanitizeQueryValidation(
      warm.queryValidation,
      querySource,
      queries,
    ),
  };
}

function sanitizeCold(cold, querySource, queries) {
  if (!isRecord(cold)) return null;
  return {
    samples: Array.isArray(cold.samples)
      ? cold.samples.map((sample, index) =>
          sanitizeSample(sample, querySource, index, queries),
        )
      : [],
    stats: sanitizeStats(cold.stats, queries),
  };
}

function redactFailures(failures, queries) {
  return (Array.isArray(failures) ? failures : []).map((failure) => ({
    phase: sanitizeSafeValue(failure?.phase, queries, "phase"),
    message: sanitizeSafeValue(failure?.message, queries, "message"),
  }));
}

function queryCases(queries, querySource) {
  return (Array.isArray(queries) ? queries : []).map((query, index) =>
    queryEvidence(query, querySource, index),
  );
}

function defaultQueryExpectations() {
  return DEFAULT_QUERY_SPECS.map((spec, index) =>
    queryEvidence(spec.query, "b4-synthetic-fixture", index),
  );
}

module.exports = {
  defaultQueryExpectations,
  queryCases,
  queryEvidence,
  redactFailures,
  redactText,
  sanitizeSafeValue,
  sanitizeCold,
  sanitizeQueryValidation,
  sanitizeSample,
  sanitizeWarm,
};
