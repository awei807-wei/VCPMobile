"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { summarizeTopicSet } = require("./scale-gate.cjs");

const A = "a".repeat(64);
const B = "b".repeat(64);

test("scale summaries require the exact topic identity set and wire hashes", () => {
  const summary = summarizeTopicSet(
    [
      { id: "topic-b" },
      { id: "topic-a" },
    ],
    ["topic-a", "topic-b"],
    [
      { id: "topic-b", configHash: A, contentHash: "" },
      { id: "topic-a", config_hash: B, content_hash: A },
    ],
  );
  assert.equal(summary.exactTopicSet, true);
  assert.equal(summary.canonicalHashesPresent, true);
  assert.equal(summary.topicCount, 2);
  assert.match(summary.topicSetHash, /^[a-f0-9]{8}…[a-f0-9]{4}$/);
});

test("scale summaries reject debug hash rows with a different topic set", () => {
  const summary = summarizeTopicSet(
    [{ id: "topic-a" }, { id: "topic-b" }],
    ["topic-a", "topic-b"],
    [
      { id: "topic-a", configHash: A, contentHash: B },
      { id: "unrelated", configHash: A, contentHash: B },
    ],
  );
  assert.equal(summary.exactTopicSet, true);
  assert.equal(summary.canonicalHashesPresent, false);
});

test("equal counts cannot hide a missing scale topic", () => {
  const summary = summarizeTopicSet(
    [
      { id: "topic-a", configHash: A, contentHash: B },
      { id: "unrelated", configHash: A, contentHash: B },
    ],
    ["topic-a", "topic-b"],
  );
  assert.equal(summary.topicCount, 2);
  assert.equal(summary.exactTopicSet, false);
});

test("malformed canonical hashes fail the scale gate", () => {
  const summary = summarizeTopicSet(
    [{ id: "topic-a" }],
    ["topic-a"],
    [{ id: "topic-a", configHash: A, contentHash: "invalid" }],
  );
  assert.equal(summary.exactTopicSet, true);
  assert.equal(summary.canonicalHashesPresent, false);
});
