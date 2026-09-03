"use strict";

const crypto = require("node:crypto");
const {
  compactHash,
  loadTopicMessages,
  messageDigest,
} = require("./android-state.cjs");

const SHA256 = /^[a-f0-9]{64}$/;
const CONTENT_HASH = /^(?:|[a-f0-9]{64})$/;

function digest(value) {
  return crypto
    .createHash("sha256")
    .update(JSON.stringify(value))
    .digest("hex");
}

function topicHash(topic, camelName, snakeName) {
  const value = topic?.[camelName] ?? topic?.[snakeName] ?? null;
  return typeof value === "string" ? value : null;
}

function sameSortedIds(left, right) {
  return JSON.stringify([...left].sort()) === JSON.stringify([...right].sort());
}

function summarizeTopicSet(topics, expectedTopicIds, canonicalHashRows) {
  const list = Array.isArray(topics) ? topics : [];
  const expected = [...expectedTopicIds];
  const actual = list.map((topic) => topic?.id || "");
  const exactTopicSet = sameSortedIds(actual, expected);
  const hashList = Array.isArray(canonicalHashRows) ? canonicalHashRows : [];
  const canonicalIds = hashList.map((topic) => topic?.id || "");
  const canonicalHashTopicSet = sameSortedIds(canonicalIds, actual);
  const canonicalHashesPresent =
    exactTopicSet &&
    canonicalHashTopicSet &&
    hashList.every((topic) => {
      const configHash = topicHash(topic, "configHash", "config_hash");
      const contentHash = topicHash(topic, "contentHash", "content_hash");
      return (
        SHA256.test(configHash || "") &&
        typeof contentHash === "string" &&
        CONTENT_HASH.test(contentHash)
      );
    });
  const normalized = hashList
    .map((topic) => ({
      id: topic?.id || "",
      configHash: topicHash(topic, "configHash", "config_hash"),
      contentHash: topicHash(topic, "contentHash", "content_hash"),
    }))
    .sort((left, right) => left.id.localeCompare(right.id));
  return {
    exactTopicSet,
    canonicalHashesPresent,
    topicCount: list.length,
    topicSetHash: compactHash(digest([...actual].sort())),
    topicStateHash: compactHash(digest(normalized)),
  };
}

function messageContentHash(message) {
  return message?.content_hash ?? message?.contentHash ?? null;
}

function exactScaleMessages(actual, expected, fingerprint) {
  if (actual.length !== expected.length) return false;
  return actual.every((message, index) => {
    const source = expected[index];
    return (
      message?.id === source?.id &&
      messageContentHash(message) === fingerprint(source)
    );
  });
}

async function inspectScaleFixture(context) {
  const scaleOwner = context.fixture.owners.find(
    (item) =>
      item.ownerType === "agent" &&
      item.ownerId === context.fixture.ids.scaleOwner,
  );
  if (!scaleOwner) throw new Error("scale fixture owner is missing");
  const topics = await context.runtime.cdp.cdp.invoke("get_topics", {
    ownerType: scaleOwner.ownerType,
    ownerId: scaleOwner.ownerId,
  });
  const canonicalHashRows = await context.runtime.cdp.cdp.invoke(
    "debug_get_wire14_scale_topic_hashes",
    {
      ownerType: scaleOwner.ownerType,
      ownerId: scaleOwner.ownerId,
    },
  );
  const summary = summarizeTopicSet(
    topics,
    scaleOwner.topics.map((topic) => topic.id),
    canonicalHashRows,
  );
  const firstTopic = scaleOwner.topics[0];
  const actualMessages = firstTopic
    ? await loadTopicMessages(
        context.runtime.cdp.cdp,
        scaleOwner.ownerType,
        scaleOwner.ownerId,
        firstTopic.id,
      )
    : [];
  const expectedMessages = firstTopic
    ? context.fixture.histories.get(
        `${scaleOwner.ownerId}\0${firstTopic.id}`,
      ) || []
    : [];
  return {
    ...summary,
    exactMessages: exactScaleMessages(
      actualMessages,
      expectedMessages,
      context.runtime.desktop.computeMessageFingerprint,
    ),
    messageCount: actualMessages.length,
    messageStateHash: compactHash(messageDigest(actualMessages)),
  };
}

module.exports = {
  inspectScaleFixture,
  summarizeTopicSet,
};
