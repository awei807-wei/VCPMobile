"use strict";

const {
  DEBUG_SENTINEL_KEY,
  DEBUG_PACKAGE,
  addPreciseReverse,
  assertDebugPackage,
  codedError,
  createMessageId,
  identityFromSearchResult,
  resolveIdentityInput,
} = require("./helper-stream-e2e-support.cjs");
const {
  installHarnessState,
  invokeChannel,
  readHarnessState,
  waitForState,
} = require("./helper-stream-e2e-channel.cjs");
const { sanitizeEvidence, sanitizeResult, writeEvidence } = require("./helper-stream-e2e-evidence.cjs");
const { startLoopbackSseServer } = require("./helper-stream-e2e-server.cjs");
const { cleanupResources } = require("./helper-stream-e2e-cleanup.cjs");
const { connectAndroidCdp, DEFAULT_CDP_TIMEOUT_MS } = require("./wire14/android-cdp.cjs");
const { ensureSingleDevice, runAdb } = require("./scripts/adb-env.cjs");

function buildContext(identity) {
  const context = {
    ownerType: identity.ownerType,
    ownerId: identity.ownerId,
    topicId: identity.topicId,
    agentName: "Android helper E2E",
  };
  if (identity.ownerType === "agent") context.agentId = identity.ownerId;
  return context;
}

async function resolveIdentity(options, cdp) {
  const direct = resolveIdentityInput(options);
  if (direct) return direct;
  const page = await cdp.invoke("search_messages_fts", {
    filter: { query: options.searchQuery || "全", limit: 1, sort: "time" },
  });
  return identityFromSearchResult(page);
}

function summarizeRecovery(value, identity) {
  const index = value?.lastEventIndex ?? value?.last_event_index;
  const generation = value?.helperGeneration;
  const context = value?.context;
  return {
    status: value?.status,
    contentMatchesA: value?.content === "A",
    indexPresent: Number.isSafeInteger(index) && index >= 0,
    helperGenerationPresent: Number.isSafeInteger(generation) && generation > 0,
    helperGeneration: generation,
    identityMatches: context === undefined || (
      context?.ownerType === identity.ownerType &&
      context?.ownerId === identity.ownerId && context?.topicId === identity.topicId
    ),
  };
}

function assertRecovery(value, summary) {
  if (summary.status !== "streaming" || !summary.contentMatchesA ||
      !summary.indexPresent || !summary.helperGenerationPresent) {
    throw codedError("RECOVERY_CONTRACT_FAILED", "恢复响应契约失败");
  }
  if (value?.helperGeneration !== summary.helperGeneration) {
    throw codedError("RECOVERY_GENERATION_FAILED", "恢复 generation 无效");
  }
}

function matchingActiveRow(rows, identity, messageId) {
  return (Array.isArray(rows) ? rows : []).some((row) =>
    row?.msgId === messageId && row?.ownerType === identity.ownerType &&
    row?.ownerId === identity.ownerId && row?.topicId === identity.topicId);
}

async function deleteGeneratedRecord(cdp, identity, messageId) {
  await cdp.invoke("delete_messages", {
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
    msgIds: [messageId],
  });
  return true;
}

function createDependencies(dependencies) {
  return {
    runAdb: dependencies.runAdb || runAdb,
    ensureDevice: dependencies.ensureDevice || ensureSingleDevice,
    connect: dependencies.connect || connectAndroidCdp,
    server: dependencies.server || startLoopbackSseServer,
  };
}

function createRawEvidence() {
  return {
    ok: false,
    packageDebug: DEBUG_PACKAGE,
    identity: {},
    phases: {},
    server: {},
    cleanup: {},
    fixture: { resetAndReinjectB4Required: true },
  };
}

async function prepareRuntime(options, deps, context) {
  assertDebugPackage(DEBUG_PACKAGE);
  deps.ensureDevice();
  context.server = await deps.server();
  context.reverse = addPreciseReverse(context.server.port, { runAdb: deps.runAdb });
  if (options.launch) {
    deps.runAdb(["shell", "monkey", "-p", DEBUG_PACKAGE, "1"], { allowFailure: false });
  }
  context.cdp = await deps.connect(DEFAULT_CDP_TIMEOUT_MS);
  await installHarnessState(context.cdp.cdp);
  context.callbacksInstalled = true;
}

function buildStreamPayload(server, identity, messageId) {
  return {
    vcpUrl: server.baseUrl,
    vcpApiKey: DEBUG_SENTINEL_KEY,
    messages: [{ role: "user", content: "android-helper-e2e" }],
    modelConfig: { model: "android-helper-e2e", stream: true, temperature: 0 },
    messageId,
    context: buildContext(identity),
  };
}

async function startOriginalStream(options, context, identity, messageId, expected) {
  const payload = buildStreamPayload(context.server, identity, messageId);
  await invokeChannel(context.cdp.cdp, "original", "sendToVCP", { payload }, expected);
  await context.server.waitFor("accepted", options.timeoutMs);
  await context.server.waitFor("a", options.timeoutMs);
  await waitForState(
    context.cdp.cdp,
    (state) => state.channels.original?.some((event) => event.type === "aurora"),
    options.timeoutMs,
    "START_STREAM_TIMEOUT",
  );
}

async function recoverAndResume(options, context, identity, messageId, expected) {
  const recoveryValue = await context.cdp.cdp.invoke("recover_active_generation", {
    msgId: messageId,
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
  });
  const recovery = summarizeRecovery(recoveryValue, identity);
  assertRecovery(recoveryValue, recovery);
  await invokeChannel(context.cdp.cdp, "resume", "resume_stream", {
    msgId: messageId,
    topicId: identity.topicId,
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    expectedGeneration: recovery.helperGeneration,
    lastEventIndex: -1,
  }, expected);
  await waitForState(
    context.cdp.cdp,
    (state) => (state.channels.resume || []).some((event) => event.type === "aurora"),
    options.timeoutMs,
    "RESUME_REPLAY_TIMEOUT",
  );
  return recovery;
}

async function observeResumeAndInterrupt(options, context, identity, messageId) {
  context.server.releaseB();
  await context.server.waitFor("b", options.timeoutMs);
  const resumedState = await waitForState(
    context.cdp.cdp,
    (state) => (state.channels.resume || []).filter((event) => event.type === "aurora").length > 1,
    options.timeoutMs,
    "RESUME_STREAM_TIMEOUT",
  );
  const interruptValue = await context.cdp.cdp.invoke("interruptRequest", {
    messageId: messageId,
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
  });
  await context.server.waitFor("cancelled", options.timeoutMs);
  await waitForState(
    context.cdp.cdp,
    (state) => state.results.original?.settled === true && state.results.resume?.settled === true,
    options.timeoutMs,
    "INTERRUPT_SETTLEMENT_TIMEOUT",
  );
  return {
    resumeEvents: resumedState.channels.resume || [],
    interruptSuccess: interruptValue?.success === true,
    settled: await readHarnessState(context.cdp.cdp),
  };
}

function recordInterruptResults(raw, observation) {
  const resumeEvents = observation.resumeEvents;
  const originalResult = sanitizeResult(observation.settled.results.original?.result || {});
  const resumeResult = sanitizeResult(observation.settled.results.resume?.result || {});
  raw.phases.resume = {
    replayObserved: resumeEvents.some((event) => event.type === "aurora" && event.hasChunk),
    laterEventObserved: resumeEvents.filter((event) => event.type === "aurora").length > 1,
    identityMatches: resumeEvents.length > 0 && resumeEvents.every((event) => event.identityMatches),
    result: resumeResult,
  };
  raw.phases.interrupt = {
    success: observation.interruptSuccess,
    sseCancelled: true,
    originalSkipped: originalResult.finalization === "skipped",
    resumeCancelled: resumeResult.finishReason === "cancelled_by_user",
  };
}

async function finalizeRun(context, identity, messageId, raw) {
  const finalValue = await context.cdp.cdp.invoke("recover_active_generation", {
    msgId: messageId,
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
  }).catch(() => ({ status: "failed" }));
  const activeRows = await context.cdp.cdp.invoke("get_active_generations");
  const recoveryStatus = finalValue?.status || null;
  raw.phases.final = {
    recoveryStatus,
    nonStreaming: recoveryStatus !== "streaming",
    activeRowAbsent: !matchingActiveRow(activeRows, identity, messageId),
  };
  if (!raw.phases.interrupt.success || !raw.phases.interrupt.originalSkipped ||
      !raw.phases.interrupt.resumeCancelled || !raw.phases.final.nonStreaming ||
      !raw.phases.final.activeRowAbsent || !raw.phases.resume.identityMatches) {
    throw codedError("INTERRUPT_CONTRACT_FAILED", "中断终态契约失败");
  }
}

async function cleanupRun(context, deps, identity, messageId, raw) {
  if (context.cdp && identity && messageId) {
    try {
      raw.cleanup.databaseRecordDeleted = await deleteGeneratedRecord(
        context.cdp.cdp,
        identity,
        messageId,
      );
    } catch {
      raw.cleanup.databaseRecordDeleted = false;
    }
  }
  const issues = await cleanupResources(context, { runAdb: deps.runAdb });
  raw.cleanup.issues = issues;
  raw.cleanup.cdpCallbacksRemoved = context.callbacksRemoved === true;
  raw.cleanup.reverseRemoved = context.reverseRemoved === true;
  raw.cleanup.serverStopped = context.serverStopped === true;
  if (issues.length > 0) raw.ok = false;
  raw.server.contracts = context.server?.snapshotContracts?.() || [];
}

async function runE2e(options, dependencies = {}) {
  const deps = createDependencies(dependencies);
  const context = { cdp: null, reverse: null, server: null, callbacksInstalled: false };
  const raw = createRawEvidence();
  let identity = null;
  let messageId = null;
  try {
    await prepareRuntime(options, deps, context);
    identity = await resolveIdentity(options, context.cdp.cdp);
    messageId = createMessageId();
    raw.identity = { ownerType: identity.ownerType, ownerIdPresent: true, topicIdPresent: true, allFieldsMatch: true };
    const expected = { ...identity, messageId };
    await startOriginalStream(options, context, identity, messageId, expected);
    raw.phases.recovery = await recoverAndResume(options, context, identity, messageId, expected);
    const observation = await observeResumeAndInterrupt(options, context, identity, messageId);
    recordInterruptResults(raw, observation);
    await finalizeRun(context, identity, messageId, raw);
    raw.server.sseCancelled = true;
    raw.ok = true;
  } catch (error) {
    raw.failure = { code: error?.code || "E2E_FAILED" };
  } finally {
    await cleanupRun(context, deps, identity, messageId, raw);
  }
  const evidence = sanitizeEvidence(raw);
  if (options.evidencePath) await writeEvidence(options.evidencePath, evidence);
  return evidence;
}

module.exports = { runE2e, summarizeRecovery };
