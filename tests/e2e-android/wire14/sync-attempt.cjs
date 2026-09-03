"use strict";

const { sampleAndroidMemory } = require("./android-state.cjs");
const { sampleDesktopMemory } = require("./desktop-state.cjs");
const { summarizeEvents, summarizeStatus } = require("./evidence.cjs");

const TERMINAL_STATUSES = new Set([
  "completed",
  "completed_with_warnings",
  "error",
]);
const SUCCESS_STATUSES = new Set(["completed", "completed_with_warnings"]);

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function drainEvents(cdp, events) {
  const next = await cdp.readEvents();
  if (Array.isArray(next)) events.push(...next);
}

function errorCodes(summary) {
  return [
    ...new Set(
      summary.records
        .map((record) => record.error?.code)
        .filter((code) => typeof code === "string"),
    ),
  ].slice(0, 8);
}

function snapshotProtocolCounters(desktop) {
  return (
    desktop?.snapshotProtocolCounters?.() || {
      versionChecksReceived: 0,
      versionAcksSent: 0,
      finalAcksSent: 0,
      finalCompletionsReceived: 0,
      finalAckIdentities: [],
      finalCompletionIdentities: [],
    }
  );
}

function protocolDelta(before, after) {
  const finalAckIdentities = after.finalAckIdentities.slice(
    before.finalAckIdentities.length,
  );
  const finalCompletionIdentities = after.finalCompletionIdentities.slice(
    before.finalCompletionIdentities.length,
  );
  return {
    versionChecksReceived: Math.max(
      0,
      after.versionChecksReceived - before.versionChecksReceived,
    ),
    versionAcksSent: Math.max(
      0,
      after.versionAcksSent - before.versionAcksSent,
    ),
    finalAcksSent: Math.max(0, after.finalAcksSent - before.finalAcksSent),
    finalCompletionsReceived: Math.max(
      0,
      after.finalCompletionsReceived - before.finalCompletionsReceived,
    ),
    finalIdentityMatched:
      finalAckIdentities.length === 1 &&
      finalCompletionIdentities.length === 1 &&
      finalAckIdentities[0] === finalCompletionIdentities[0],
  };
}

function protocolOutcome(counters) {
  const handshakeObserved =
    counters.versionChecksReceived > 0 &&
    counters.versionChecksReceived === counters.versionAcksSent;
  const finalAckObserved =
    counters.finalCompletionsReceived === 1 &&
    counters.finalAcksSent === 1 &&
    counters.finalIdentityMatched === true;
  return { handshakeObserved, finalAckObserved };
}

async function executeAttempt(cdp, name, options, hooks = {}) {
  const { timeoutMs, pollMs, signal } = options;
  const events = [];
  const startedAt = Date.now();
  let sessionId = null;
  let lastStatus = null;
  let sawActiveStatus = false;
  let androidPeak = null;
  let desktopPeak = null;
  const protocolBefore = snapshotProtocolCounters(hooks.runtime?.desktop);
  const sampleMemory = async () => {
    const [android, desktop] = await Promise.all([
      Promise.resolve(sampleAndroidMemory()),
      sampleDesktopMemory(hooks.runtime?.desktop),
    ]);
    if (Number.isSafeInteger(android))
      androidPeak = Math.max(androidPeak || 0, android);
    if (Number.isSafeInteger(desktop?.peakRssBytes)) {
      desktopPeak = Math.max(desktopPeak || 0, desktop.peakRssBytes);
    } else if (Number.isSafeInteger(desktop?.rssBytes)) {
      desktopPeak = Math.max(desktopPeak || 0, desktop.rssBytes);
    }
  };

  if (signal?.aborted) {
    return {
      name,
      started: false,
      terminalStatus: "error",
      error: "interrupted",
      durationMs: 0,
      events: summarizeEvents(events),
    };
  }

  try {
    sessionId = await cdp.invoke("start_manual_sync");
  } catch {
    return {
      name,
      started: false,
      terminalStatus: "error",
      error: "start_manual_sync_failed",
      durationMs: Date.now() - startedAt,
      events: summarizeEvents(events),
    };
  }

  while (Date.now() - startedAt < timeoutMs && !signal?.aborted) {
    try {
      await drainEvents(cdp, events);
      lastStatus = summarizeStatus(await cdp.invoke("get_sync_status")).status;
      if (lastStatus && lastStatus !== "disconnected") sawActiveStatus = true;
      if (typeof hooks.onStatus === "function")
        await hooks.onStatus(lastStatus, events);
      await sampleMemory();
    } catch {
      lastStatus = null;
    }
    if (
      TERMINAL_STATUSES.has(lastStatus) &&
      (sawActiveStatus || Date.now() - startedAt > 1_000)
    )
      break;
    await sleep(pollMs);
  }
  await drainEvents(cdp, events).catch(() => {});
  await sampleMemory();
  const summary = summarizeEvents(events);
  const terminalStatus = TERMINAL_STATUSES.has(lastStatus)
    ? lastStatus
    : summary.finalStatus;
  const protocolCounters = protocolDelta(
    protocolBefore,
    snapshotProtocolCounters(hooks.runtime?.desktop),
  );
  const { handshakeObserved, finalAckObserved } =
    protocolOutcome(protocolCounters);
  return {
    name,
    started: true,
    sessionId: Number.isSafeInteger(sessionId) ? sessionId : null,
    terminalStatus: terminalStatus || null,
    timedOut: !TERMINAL_STATUSES.has(terminalStatus),
    durationMs: Date.now() - startedAt,
    statusAtLastPoll: lastStatus,
    handshakeObserved,
    finalAckObserved,
    protocolCounters,
    peakAndroidMemoryBytes: androidPeak,
    peakDesktopMemoryBytes: desktopPeak,
    errorCodes: errorCodes(summary),
    events: summary,
  };
}

async function waitForSessionIdle(cdp, pollMs, signal) {
  for (let index = 0; index < 40 && !signal?.aborted; index += 1) {
    try {
      if (!(await cdp.invoke("is_sync_active"))) return true;
    } catch {
      return false;
    }
    await sleep(pollMs);
  }
  return false;
}

async function runSyncAttempt(runtime, name, options, hooks = {}) {
  const attempt = await executeAttempt(
    runtime.cdp.cdp,
    name,
    {
      timeoutMs: options.timeoutMs,
      pollMs: options.pollMs,
      signal: runtime.abortController.signal,
    },
    { ...hooks, runtime },
  );
  await waitForSessionIdle(
    runtime.cdp.cdp,
    options.pollMs,
    runtime.abortController.signal,
  );
  return attempt;
}

function isSuccess(attempt) {
  return Boolean(
    attempt?.started &&
    attempt.timedOut !== true &&
    SUCCESS_STATUSES.has(attempt.terminalStatus) &&
    attempt.handshakeObserved === true &&
    attempt.finalAckObserved === true,
  );
}

module.exports = {
  TERMINAL_STATUSES,
  SUCCESS_STATUSES,
  executeAttempt,
  runSyncAttempt,
  waitForSessionIdle,
  isSuccess,
  protocolDelta,
  protocolOutcome,
};
