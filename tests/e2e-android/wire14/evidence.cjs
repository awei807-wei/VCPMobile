"use strict";

const PHASES = new Set([
  "owner_metadata",
  "avatar_metadata",
  "topic_metadata",
  "topic_validation",
  "messages",
  "finalize",
]);
const STATUSES = new Set([
  "connecting",
  "open",
  "disconnected",
  "retrying",
  "stopped",
  "error",
  "completed",
  "completed_with_warnings",
]);
const HASH_KEYS = new Set([
  "hash",
  "configHash",
  "contentHash",
  "messageHash",
  "sourceHash",
]);
const SECRET_KEYS = new Set([
  "token",
  "synctoken",
  "authorization",
  "apikey",
  "password",
]);
const MESSAGE_KEYS = new Set(["content", "messagebody", "rawmessage"]);
const PATH_KEYS = new Set(["path", "filepath", "internalpath", "src"]);

function safeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function maskHash(value) {
  if (typeof value !== "string" || value.length === 0) return null;
  const normalized = value.toLowerCase();
  if (!/^[a-f0-9]{16,128}$/.test(normalized)) return "present";
  return `${normalized.slice(0, 8)}…${normalized.slice(-4)}`;
}

function safeEnum(value, known) {
  return typeof value === "string" && known.has(value) ? value : null;
}

function collectHashes(value, hashes, depth = 0) {
  if (!value || typeof value !== "object" || depth > 3) return;
  for (const [key, child] of Object.entries(value)) {
    if (HASH_KEYS.has(key)) {
      const masked = maskHash(child);
      if (masked) hashes.add(masked);
      continue;
    }
    if (child && typeof child === "object")
      collectHashes(child, hashes, depth + 1);
  }
}

function summarizeError(error) {
  if (!error || typeof error !== "object") return null;
  const result = {};
  if (
    typeof error.code === "string" &&
    /^[A-Z0-9_:-]{1,80}$/.test(error.code)
  ) {
    result.code = error.code;
  }
  if (
    typeof error.origin === "string" &&
    /^[a-z0-9_:-]{1,40}$/.test(error.origin)
  ) {
    result.origin = error.origin;
  }
  if (typeof error.stage === "string" && PHASES.has(error.stage))
    result.stage = error.stage;
  if (typeof error.retry === "string" && /^[a-z]{1,20}$/.test(error.retry))
    result.retry = error.retry;
  if (Array.isArray(error.failedTopicIds))
    result.failedTopicCount = error.failedTopicIds.length;
  return Object.keys(result).length > 0 ? result : null;
}

function summarizeEvent(event, hashes) {
  const payload = event?.payload;
  const result = {
    event: typeof event?.name === "string" ? event.name : "unknown",
  };
  if (!payload || typeof payload !== "object") return result;

  const phase = safeEnum(payload.phase, PHASES);
  const status = safeEnum(payload.status, STATUSES);
  if (phase) result.phase = phase;
  if (status) result.status = status;
  for (const key of [
    "sessionId",
    "attemptId",
    "total",
    "completed",
    "successfulTopics",
    "totalTopics",
    "failedTopics",
    "legacyAttachmentWarnings",
  ]) {
    const number = safeInteger(payload[key]);
    if (number !== null) result[key] = number;
  }
  const error = summarizeError(payload.error);
  if (error) result.error = error;
  collectHashes(payload, hashes);
  if (result.event === "vcp-log" && typeof payload.message === "string") {
    if (/handshake|version[_ -]?ack|protocol version/i.test(payload.message)) {
      result.marker = "handshake";
    } else if (
      /final[_ -]?ack|ack[_ -]?final|最终.*确认/i.test(payload.message)
    ) {
      result.marker = "final_ack";
    } else {
      result.marker = /ack|phase|wire/i.test(payload.message)
        ? "protocol"
        : "operator";
    }
  }
  return result;
}

function summarizeEvents(events) {
  const hashes = new Set();
  const records = Array.isArray(events)
    ? events.map((event) => summarizeEvent(event, hashes))
    : [];
  const statuses = records
    .filter((record) => record.status)
    .map((record) => record.status);
  const statusCounts = Object.fromEntries(
    [...new Set(statuses)].map((status) => [
      status,
      statuses.filter((item) => item === status).length,
    ]),
  );
  const phases = [
    ...new Set(
      records.filter((record) => record.phase).map((record) => record.phase),
    ),
  ];
  const finalRecord = [...records].reverse().find((record) => record.status);
  return {
    eventCount: records.length,
    phases,
    statuses: [...new Set(statuses)],
    statusCounts,
    finalStatus: finalRecord?.status || null,
    records,
    hashes: [...hashes],
    protocolMarkers: records.filter((record) => record.marker === "protocol")
      .length,
    handshakeObserved: records.some((record) => record.marker === "handshake"),
    finalAckObserved: records.some((record) => record.marker === "final_ack"),
  };
}

function summarizeStatus(value) {
  if (typeof value !== "string") return { rawType: typeof value, status: null };
  const status = value.trim();
  return {
    rawType: "string",
    status: STATUSES.has(status) ? status : null,
  };
}

const SCENARIO_STATUSES = new Set(["passed", "failed", "unsupported"]);

function summarizeScenario(scenario) {
  const result = {
    name: typeof scenario?.name === "string" ? scenario.name : "unknown",
    status: SCENARIO_STATUSES.has(scenario?.status)
      ? scenario.status
      : "failed",
  };
  for (const key of [
    "attemptCount",
    "topicCount",
    "messageCount",
    "expectedTopicCount",
    "droppedConnections",
    "retryCount",
    "peakAndroidMemoryBytes",
    "peakDesktopMemoryBytes",
    "transportOperations",
    "httpRequests",
  ]) {
    const number = safeInteger(scenario?.[key]);
    if (number !== null) result[key] = number;
  }
  for (const key of [
    "handshakeObserved",
    "finalAckObserved",
    "finalAckDropped",
    "finalAckRejected",
    "noResurrection",
    "crossOwnerIsolated",
    "desktopObserved",
    "androidObserved",
    "avatarObserved",
    "groupAvatarObserved",
    "validAttachmentObserved",
    "missingBinaryAccepted",
    "invalidRejected",
    "invalidCodeObserved",
    "foregrounded",
    "backgrounded",
    "processRestarted",
    "injected",
    "recovered",
    "noOp",
  ]) {
    if (typeof scenario?.[key] === "boolean") result[key] = scenario[key];
  }
  if (
    typeof scenario?.reason === "string" &&
    /^[a-z0-9_:-]{1,80}$/.test(scenario.reason)
  ) {
    result.reason = scenario.reason;
  }
  if (Array.isArray(scenario?.errorCodes)) {
    result.errorCodes = scenario.errorCodes
      .filter(
        (code) => typeof code === "string" && /^[A-Z0-9_:-]{1,80}$/.test(code),
      )
      .slice(0, 8);
  }
  if (Array.isArray(scenario?.hashes)) {
    result.hashes = scenario.hashes
      .filter((hash) => typeof hash === "string")
      .map(maskHash)
      .filter(Boolean)
      .slice(0, 8);
  }
  return result;
}

function summarizeScenarios(scenarios) {
  const records = Array.isArray(scenarios)
    ? scenarios.map(summarizeScenario)
    : [];
  return {
    count: records.length,
    passed: records.filter((record) => record.status === "passed").length,
    failed: records.filter((record) => record.status === "failed").length,
    unsupported: records.filter((record) => record.status === "unsupported")
      .length,
    records,
  };
}

function auditEvidencePayload(value, secrets = []) {
  const findings = {
    tokenOutput: false,
    messageOutput: false,
    piiOutput: false,
    pathOutput: false,
  };
  const secretValues = secrets.filter(
    (secret) => typeof secret === "string" && secret.length >= 8,
  );
  const visit = (child, key = "") => {
    const normalizedKey = key.toLowerCase();
    if (SECRET_KEYS.has(normalizedKey)) findings.tokenOutput = true;
    if (MESSAGE_KEYS.has(normalizedKey)) findings.messageOutput = true;
    if (PATH_KEYS.has(normalizedKey)) findings.pathOutput = true;
    if (typeof child === "string") {
      if (
        /bearer\s+[a-z0-9._~+/=-]+/i.test(child) ||
        secretValues.some((secret) => child.includes(secret))
      ) {
        findings.tokenOutput = true;
      }
      if (/file:\/\/|\/(?:home|tmp)\/|\b[A-Za-z]:[\\/]/.test(child)) {
        findings.pathOutput = true;
      }
      if (
        /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/i.test(child) ||
        /\b1[3-9]\d{9}\b/.test(child)
      ) {
        findings.piiOutput = true;
      }
      return;
    }
    if (Array.isArray(child)) {
      child.forEach((item) => visit(item));
      return;
    }
    if (child && typeof child === "object") {
      Object.entries(child).forEach(([childKey, item]) =>
        visit(item, childKey),
      );
    }
  };
  visit(value);
  return {
    ...findings,
    ok: !Object.values(findings).some(Boolean),
  };
}

module.exports = {
  auditEvidencePayload,
  summarizeEvents,
  summarizeStatus,
  summarizeScenario,
  summarizeScenarios,
};
