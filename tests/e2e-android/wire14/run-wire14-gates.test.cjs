"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  attemptMeetsGate,
  expectedFailureObserved,
} = require("./run-wire14.cjs");

function failedAttempt(overrides = {}) {
  return {
    started: true,
    timedOut: false,
    terminalStatus: "error",
    handshakeObserved: true,
    finalAckObserved: false,
    expectedFailure: "MOBILE_ATTACHMENT_INVALID",
    errorCodes: ["MOBILE_ATTACHMENT_INVALID"],
    ...overrides,
  };
}

test("expected failures must match the declared structured error", () => {
  assert.equal(expectedFailureObserved(failedAttempt()), true);
  assert.equal(
    expectedFailureObserved(
      failedAttempt({ errorCodes: ["SYNC_ATTEMPT_FAILED"] }),
    ),
    false,
  );
  assert.equal(
    expectedFailureObserved(failedAttempt({ terminalStatus: "completed" })),
    false,
  );
});

test("ordinary attempts cannot bypass the full protocol success gate", () => {
  assert.equal(
    attemptMeetsGate({
      started: true,
      timedOut: false,
      terminalStatus: "completed",
      handshakeObserved: true,
      finalAckObserved: false,
      errorCodes: [],
    }),
    false,
  );
});
