"use strict";

const crypto = require("node:crypto");

const OWNER_A = "wire14-e2e-owner-a";
const OWNER_B = "wire14-e2e-owner-b";
const GROUP_OWNER = "wire14-e2e-group-owner";
const SCALE_OWNER = "wire14-e2e-scale-owner";
const SHARED_TOPIC = "wire14-shared-topic";
const LINUX_TOPIC = "wire14-linux-topic";
const ANDROID_TOPIC = "wire14-android-topic";
const MESSAGE_TOMBSTONE_TOPIC = "wire14-message-tombstone";
const TOPIC_TOMBSTONE_TOPIC = "wire14-topic-tombstone";
const ATTACHMENT_TOPIC = "wire14-attachment-topic";
const LIFECYCLE_TOPIC = "wire14-lifecycle-topic";

const SHARED_MESSAGE = "wire14-shared-message";
const LINUX_BASE_MESSAGE = "wire14-linux-base-message";
const ANDROID_BASE_MESSAGE = "wire14-android-base-message";
const MESSAGE_TOMBSTONE = "wire14-message-tombstone-item";
const TOPIC_TOMBSTONE_MESSAGE = "wire14-topic-tombstone-item";
const ATTACHMENT_MESSAGE = "wire14-attachment-message";
const LIFECYCLE_MESSAGE = "wire14-lifecycle-message";

const ATTACHMENT_BYTES = Buffer.from(
  "Synthetic Wire 1.4 attachment bytes.\n",
  "utf8",
);
const ATTACHMENT_HASH = crypto
  .createHash("sha256")
  .update(ATTACHMENT_BYTES)
  .digest("hex");
const MISSING_ATTACHMENT_HASH = crypto
  .createHash("sha256")
  .update("synthetic-missing-binary")
  .digest("hex");
const INVALID_ATTACHMENT_HASH = "not-a-sha256";
const AVATAR_BYTES = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
  "base64",
);

function topic(topicId, index, { locked = false } = {}) {
  return {
    id: topicId,
    name: `Synthetic Wire 1.4 topic ${topicId}`,
    createdAt: 1700000000000 + index,
    locked,
    unread: false,
  };
}

function message(id, content, timestamp, extra = {}) {
  return {
    id,
    role: "user",
    content,
    timestamp,
    updatedAt: timestamp,
    ...extra,
  };
}

function validAttachment() {
  return {
    type: "text/plain",
    name: "synthetic.txt",
    size: ATTACHMENT_BYTES.length,
    hash: ATTACHMENT_HASH,
    src: "",
    createdAt: 1700000000400,
  };
}

function missingAttachment() {
  return {
    type: "application/octet-stream",
    name: "missing.bin",
    size: 37,
    hash: MISSING_ATTACHMENT_HASH,
    src: "",
    createdAt: 1700000000401,
  };
}

function invalidAttachment() {
  return {
    type: "application/octet-stream",
    name: "invalid.bin",
    size: 1,
    hash: INVALID_ATTACHMENT_HASH,
    src: "",
    createdAt: 1700000000402,
  };
}

function scopedId(value, runId) {
  if (typeof runId !== "string" || runId.length === 0) return value;
  if (!/^[a-zA-Z0-9_-]{1,32}$/.test(runId))
    throw new Error("fixture runId is invalid");
  return `${value}-${runId}`;
}

function buildFixtureModel({ scaleTopics = 1794, runId = "" } = {}) {
  const scaleCount =
    Number.isSafeInteger(scaleTopics) && scaleTopics >= 0 ? scaleTopics : 1794;
  const ownerA = scopedId(OWNER_A, runId);
  const ownerB = scopedId(OWNER_B, runId);
  const groupOwner = scopedId(GROUP_OWNER, runId);
  const scaleOwner = scopedId(SCALE_OWNER, runId);
  const sharedTopic = scopedId(SHARED_TOPIC, runId);
  const linuxTopic = scopedId(LINUX_TOPIC, runId);
  const androidTopic = scopedId(ANDROID_TOPIC, runId);
  const messageTombstoneTopic = scopedId(MESSAGE_TOMBSTONE_TOPIC, runId);
  const topicTombstoneTopic = scopedId(TOPIC_TOMBSTONE_TOPIC, runId);
  const attachmentTopic = scopedId(ATTACHMENT_TOPIC, runId);
  const lifecycleTopic = scopedId(LIFECYCLE_TOPIC, runId);
  const sharedMessage = scopedId(SHARED_MESSAGE, runId);
  const linuxBaseMessage = scopedId(LINUX_BASE_MESSAGE, runId);
  const androidBaseMessage = scopedId(ANDROID_BASE_MESSAGE, runId);
  const messageTombstone = scopedId(MESSAGE_TOMBSTONE, runId);
  const topicTombstoneMessage = scopedId(TOPIC_TOMBSTONE_MESSAGE, runId);
  const attachmentMessage = scopedId(ATTACHMENT_MESSAGE, runId);
  const lifecycleMessage = scopedId(LIFECYCLE_MESSAGE, runId);
  const scaleMessage = scopedId("wire14-scale-message", runId);
  const ownerTopics = new Map([
    [
      ownerA,
      [
        sharedTopic,
        linuxTopic,
        androidTopic,
        messageTombstoneTopic,
        topicTombstoneTopic,
        attachmentTopic,
        lifecycleTopic,
      ],
    ],
    [ownerB, [sharedTopic]],
    [groupOwner, [sharedTopic]],
    [
      scaleOwner,
      Array.from({ length: scaleCount }, (_, index) =>
        scopedId(`wire14-scale-${String(index + 1).padStart(4, "0")}`, runId),
      ),
    ],
  ]);
  const histories = new Map([
    [
      `${ownerA}\0${sharedTopic}`,
      [
        message(
          sharedMessage,
          "Synthetic owner A shared topic message.",
          1700000000101,
        ),
      ],
    ],
    [
      `${ownerB}\0${sharedTopic}`,
      [
        message(
          sharedMessage,
          "Synthetic owner B shared topic message.",
          1700000000102,
        ),
      ],
    ],
    [
      `${groupOwner}\0${sharedTopic}`,
      [
        message(
          sharedMessage,
          "Synthetic group shared topic message.",
          1700000000103,
        ),
      ],
    ],
    [
      `${ownerA}\0${linuxTopic}`,
      [
        message(
          linuxBaseMessage,
          "Synthetic Linux baseline message.",
          1700000000110,
        ),
      ],
    ],
    [
      `${ownerA}\0${androidTopic}`,
      [
        message(
          androidBaseMessage,
          "Synthetic Android baseline message.",
          1700000000120,
        ),
      ],
    ],
    [
      `${ownerA}\0${messageTombstoneTopic}`,
      [
        message(
          messageTombstone,
          "Synthetic message tombstone candidate.",
          1700000000130,
        ),
      ],
    ],
    [
      `${ownerA}\0${topicTombstoneTopic}`,
      [
        message(
          topicTombstoneMessage,
          "Synthetic topic tombstone candidate.",
          1700000000140,
        ),
      ],
    ],
    [
      `${ownerA}\0${attachmentTopic}`,
      [
        message(
          attachmentMessage,
          "Synthetic attachment metadata candidate.",
          1700000000150,
          {
            attachments: [validAttachment(), missingAttachment()],
          },
        ),
      ],
    ],
    [
      `${ownerA}\0${lifecycleTopic}`,
      [
        message(
          lifecycleMessage,
          "Synthetic lifecycle candidate.",
          1700000000160,
        ),
      ],
    ],
  ]);
  if (scaleCount > 0) {
    const firstScaleTopic = scopedId("wire14-scale-0001", runId);
    histories.set(`${scaleOwner}\0${firstScaleTopic}`, [
      message(scaleMessage, "Synthetic scale message.", 1700000000200),
    ]);
  }
  const owners = [...ownerTopics.entries()].map(([ownerId, topicIds]) => ({
    ownerType: ownerId === groupOwner ? "group" : "agent",
    ownerId,
    topics: topicIds.map((topicId, index) => topic(topicId, index)),
  }));
  const totalTopics = owners.reduce(
    (sum, owner) => sum + owner.topics.length,
    0,
  );
  return {
    runId,
    owners,
    histories,
    totalTopics,
    ids: {
      ownerA,
      ownerB,
      groupOwner,
      scaleOwner,
      sharedTopic,
      linuxTopic,
      androidTopic,
      messageTombstoneTopic,
      topicTombstoneTopic,
      attachmentTopic,
      lifecycleTopic,
      sharedMessage,
      linuxBaseMessage,
      androidBaseMessage,
      messageTombstone,
      topicTombstoneMessage,
      attachmentMessage,
      lifecycleMessage,
    },
    assets: {
      attachmentBytes: ATTACHMENT_BYTES,
      attachmentHash: ATTACHMENT_HASH,
      missingAttachmentHash: MISSING_ATTACHMENT_HASH,
      invalidAttachmentHash: INVALID_ATTACHMENT_HASH,
      avatarBytes: AVATAR_BYTES,
    },
  };
}

module.exports = {
  OWNER_A,
  OWNER_B,
  GROUP_OWNER,
  SCALE_OWNER,
  SHARED_TOPIC,
  LINUX_TOPIC,
  ANDROID_TOPIC,
  MESSAGE_TOMBSTONE_TOPIC,
  TOPIC_TOMBSTONE_TOPIC,
  ATTACHMENT_TOPIC,
  LIFECYCLE_TOPIC,
  SHARED_MESSAGE,
  LINUX_BASE_MESSAGE,
  ANDROID_BASE_MESSAGE,
  MESSAGE_TOMBSTONE,
  TOPIC_TOMBSTONE_MESSAGE,
  ATTACHMENT_MESSAGE,
  LIFECYCLE_MESSAGE,
  ATTACHMENT_BYTES,
  ATTACHMENT_HASH,
  MISSING_ATTACHMENT_HASH,
  INVALID_ATTACHMENT_HASH,
  AVATAR_BYTES,
  topic,
  message,
  validAttachment,
  missingAttachment,
  invalidAttachment,
  buildFixtureModel,
};
