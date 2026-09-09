"use strict";

const { DEBUG_PACKAGE, SSE_PATHS, TEST_ID } = require("./helper-stream-e2e-support.cjs");

const OWNER_TYPES = new Set(["agent", "group"]);
const RESULT_STATUSES = new Set(["streaming", "completed", "stale", "failed", "skipped"]);
const FINISH_REASONS = new Set(["cancelled_by_user", "completed", "stop", "error"]);

function safeStatus(value) {
  return typeof value === "string" && RESULT_STATUSES.has(value) ? value : null;
}

function safeFinishReason(value) {
  return typeof value === "string" && FINISH_REASONS.has(value) ? value : null;
}

function sanitizeResult(value) {
  return {
    status: safeStatus(value?.status),
    finalization: value?.finalization === "skipped" ? "skipped" : null,
    finishReason: safeFinishReason(value?.finishReason),
    streamingStarted: typeof value?.streamingStarted === "boolean" ? value.streamingStarted : null,
    hasFullContent: typeof value?.fullContent === "string",
  };
}

function sanitizeChannelEvents(events, identity, generation) {
  const expectedGeneration = Number.isSafeInteger(generation) && generation > 0 ? generation : null;
  return (Array.isArray(events) ? events : []).map((event) => ({
    type: typeof event?.type === "string" ? event.type : "unknown",
    messageIdMatches: event?.messageIdMatches === true,
    identityMatches: event?.identityMatches === true,
    generationMatches: expectedGeneration !== null && event?.generation === expectedGeneration,
    generationPresent: Number.isSafeInteger(event?.generation) && event.generation > 0,
    hasChunk: event?.hasChunk === true,
    final: event?.final === true,
    finishReason: safeFinishReason(event?.finishReason),
    expectedOwnerType: identity?.ownerType === event?.ownerType,
  }));
}

function sanitizeContract(contract) {
  const path = SSE_PATHS.includes(contract?.path) ? contract.path : "other";
  const body = contract?.body || {};
  return {
    method: contract?.method === "POST" ? "POST" : "other",
    path,
    headers: {
      authorizationPresent: contract?.headers?.authorizationPresent === true,
      contentTypePresent: contract?.headers?.contentTypePresent === true,
      acceptSsePresent: contract?.headers?.acceptSsePresent === true,
    },
    body: {
      validJson: body.validJson === true,
      stream: typeof body.stream === "boolean" ? body.stream : null,
      requestIdPresent: body.requestIdPresent === true,
      messagesArray: body.messagesArray === true,
      messageCount: Number.isSafeInteger(body.messageCount) ? body.messageCount : null,
      modelPresent: body.modelPresent === true,
    },
  };
}

function sanitizeFailure(failure) {
  const code = typeof failure?.code === "string" && /^[A-Z][A-Z0-9_]{0,63}$/.test(failure.code)
    ? failure.code
    : "E2E_FAILED";
  return { code };
}

function sanitizePhases(phases = {}) {
  const recovery = phases.recovery || {};
  const resume = phases.resume || {};
  const interrupt = phases.interrupt || {};
  const final = phases.final || {};
  return {
    recovery: {
      status: safeStatus(recovery.status),
      contentMatchesA: recovery.contentMatchesA === true,
      indexPresent: recovery.indexPresent === true,
      helperGenerationPresent: recovery.helperGenerationPresent === true,
      identityMatches: recovery.identityMatches === true,
    },
    resume: {
      replayObserved: resume.replayObserved === true,
      laterEventObserved: resume.laterEventObserved === true,
      result: sanitizeResult(resume.result),
      identityMatches: resume.identityMatches === true,
    },
    interrupt: {
      success: interrupt.success === true,
      sseCancelled: interrupt.sseCancelled === true,
      originalSkipped: interrupt.originalSkipped === true,
      resumeCancelled: interrupt.resumeCancelled === true,
    },
    final: {
      recoveryStatus: safeStatus(final.recoveryStatus),
      nonStreaming: final.nonStreaming === true,
      activeRowAbsent: final.activeRowAbsent === true,
    },
  };
}

function sanitizeCleanup(cleanup = {}) {
  return {
    databaseRecordDeleted: cleanup.databaseRecordDeleted === true,
    cdpCallbacksRemoved: cleanup.cdpCallbacksRemoved === true,
    reverseRemoved: cleanup.reverseRemoved === true,
    serverStopped: cleanup.serverStopped === true,
    issues: Array.isArray(cleanup.issues) ? cleanup.issues.map(() => "cleanup_failed") : [],
  };
}

function sanitizeEvidence(input = {}) {
  const identity = input.identity || {};
  const contracts = Array.isArray(input.server?.contracts)
    ? input.server.contracts.map(sanitizeContract)
    : [];
  return {
    schema: "vcp.android.helper.stream.e2e.v1",
    ok: input.ok === true,
    mode: "debug-only",
    packageDebug: input.packageDebug === DEBUG_PACKAGE ? DEBUG_PACKAGE : null,
    identity: {
      ownerType: OWNER_TYPES.has(identity.ownerType) ? identity.ownerType : null,
      ownerIdPresent: identity.ownerIdPresent === true,
      topicIdPresent: identity.topicIdPresent === true,
      allFieldsMatch: identity.allFieldsMatch === true,
      messageIdTestId: TEST_ID,
    },
    server: {
      requestCount: contracts.length,
      acceptedPathCount: contracts.filter((item) => SSE_PATHS.includes(item.path)).length,
      contracts,
      sseCancelled: input.server?.sseCancelled === true,
    },
    phases: sanitizePhases(input.phases),
    fixture: { resetAndReinjectB4Required: true },
    cleanup: sanitizeCleanup(input.cleanup),
    failure: input.ok === true ? null : sanitizeFailure(input.failure),
  };
}

async function writeEvidence(filePath, evidence) {
  if (typeof filePath !== "string" || filePath.trim().length === 0) {
    throw new Error("证据路径无效");
  }
  const fs = require("node:fs/promises");
  const path = require("node:path");
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, `${JSON.stringify(sanitizeEvidence(evidence), null, 2)}\n`, "utf8");
}

module.exports = {
  sanitizeChannelEvents,
  sanitizeEvidence,
  sanitizeResult,
  writeEvidence,
};
