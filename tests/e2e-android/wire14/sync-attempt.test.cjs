"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  isSuccess,
  protocolDelta,
  protocolOutcome,
} = require("./sync-attempt.cjs");

function counters(overrides = {}) {
  return {
    versionChecksReceived: 0,
    versionAcksSent: 0,
    finalAcksSent: 0,
    finalCompletionsReceived: 0,
    finalAckIdentities: [],
    finalCompletionIdentities: [],
    ...overrides,
  };
}

test("successful attempts require matched handshake and final ACK identity", () => {
  const before = counters();
  const after = counters({
    versionChecksReceived: 1,
    versionAcksSent: 1,
    finalAcksSent: 1,
    finalCompletionsReceived: 1,
    finalAckIdentities: ["7\u00001\u0000nonce"],
    finalCompletionIdentities: ["7\u00001\u0000nonce"],
  });
  const protocolCounters = protocolDelta(before, after);
  const outcome = protocolOutcome(protocolCounters);
  assert.deepEqual(outcome, {
    handshakeObserved: true,
    finalAckObserved: true,
  });
  assert.equal(
    isSuccess({
      started: true,
      timedOut: false,
      terminalStatus: "completed",
      ...outcome,
    }),
    true,
  );
});

test("a sent but mismatched final ACK cannot satisfy success", () => {
  const protocolCounters = protocolDelta(
    counters(),
    counters({
      versionChecksReceived: 1,
      versionAcksSent: 1,
      finalAcksSent: 1,
      finalCompletionsReceived: 1,
      finalAckIdentities: ["8\u00001\u0000other"],
      finalCompletionIdentities: ["7\u00001\u0000nonce"],
    }),
  );
  const outcome = protocolOutcome(protocolCounters);
  assert.equal(outcome.finalAckObserved, false);
  assert.equal(
    isSuccess({
      started: true,
      timedOut: false,
      terminalStatus: "completed",
      ...outcome,
    }),
    false,
  );
});

test("reconnect handshakes must be balanced while final ACK stays singular", () => {
  const outcome = protocolOutcome({
    versionChecksReceived: 2,
    versionAcksSent: 2,
    finalAcksSent: 1,
    finalCompletionsReceived: 1,
    finalIdentityMatched: true,
  });
  assert.deepEqual(outcome, {
    handshakeObserved: true,
    finalAckObserved: true,
  });
});
