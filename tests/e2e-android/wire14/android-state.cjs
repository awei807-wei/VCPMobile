"use strict";

const crypto = require("node:crypto");
const { DEBUG_PACKAGE, runAdb } = require("../scripts/adb-env.cjs");
const { connectAndroidCdp } = require("./android-cdp.cjs");

function compactHash(value) {
  if (typeof value !== "string" || value.length === 0) return null;
  const normalized = value.toLowerCase();
  return /^[a-f0-9]{16,128}$/.test(normalized)
    ? `${normalized.slice(0, 8)}…${normalized.slice(-4)}`
    : "present";
}

function digest(value) {
  return crypto
    .createHash("sha256")
    .update(JSON.stringify(value))
    .digest("hex");
}

function normalizedMessage(message) {
  return {
    role: message?.role || "",
    name: message?.name || null,
    content: typeof message?.content === "string" ? message.content : "",
    timestamp: Number.isSafeInteger(message?.timestamp)
      ? message.timestamp
      : null,
    updatedAt: Number.isSafeInteger(message?.updatedAt)
      ? message.updatedAt
      : null,
    attachments: Array.isArray(message?.attachments)
      ? message.attachments.map((attachment) => ({
          type: attachment?.type || "",
          name: attachment?.name || "",
          size: Number.isSafeInteger(attachment?.size) ? attachment.size : null,
          hash: attachment?.hash || null,
          attachmentOrder: attachment?.attachmentOrder ?? null,
        }))
      : [],
  };
}

function messageDigest(messages) {
  return digest(
    (Array.isArray(messages) ? messages : []).map(normalizedMessage),
  );
}

function messageAttachmentSummary(messages) {
  const attachments = (Array.isArray(messages) ? messages : []).flatMap(
    (message) =>
      Array.isArray(message?.attachments) ? message.attachments : [],
  );
  const hashes = attachments
    .map((attachment) => compactHash(attachment?.hash))
    .filter(Boolean);
  return {
    count: attachments.length,
    validHashCount: attachments.filter(
      (attachment) =>
        typeof attachment?.hash === "string" &&
        /^[a-f0-9]{64}$/i.test(attachment.hash),
    ).length,
    hashes: [...new Set(hashes)].slice(0, 8),
    totalBytes: attachments.reduce(
      (sum, attachment) =>
        sum + (Number.isSafeInteger(attachment?.size) ? attachment.size : 0),
      0,
    ),
  };
}

async function loadTopicMessages(cdp, ownerType, ownerId, topicId) {
  const messages = await cdp.invoke("load_chat_history", {
    ownerId,
    ownerType,
    topicId,
    limit: null,
    offset: null,
  });
  return Array.isArray(messages) ? messages : [];
}

async function snapshotTopics(cdp, owners, selectedTopics = []) {
  const ownerSnapshots = [];
  for (const owner of owners) {
    const topics = await cdp.invoke("get_topics", {
      ownerId: owner.ownerId,
      ownerType: owner.ownerType,
    });
    const list = Array.isArray(topics) ? topics : [];
    const selected = selectedTopics
      .filter(
        (item) =>
          item.ownerType === owner.ownerType && item.ownerId === owner.ownerId,
      )
      .map((item) => item.topicId);
    const messageSnapshots = [];
    for (const topicId of selected) {
      const messages = await loadTopicMessages(
        cdp,
        owner.ownerType,
        owner.ownerId,
        topicId,
      );
      messageSnapshots.push({
        topicLabel: "selected",
        messageCount: messages.length,
        messageHash: compactHash(messageDigest(messages)),
        attachments: messageAttachmentSummary(messages),
      });
    }
    ownerSnapshots.push({
      ownerType: owner.ownerType,
      ownerId: owner.ownerId,
      topicCount: list.length,
      topicHash: compactHash(
        digest(
          list
            .map((topic) => ({
              id: topic?.id || "",
              name: topic?.name || topic?.title || "",
              createdAt: topic?.createdAt ?? topic?.created_at ?? null,
              locked: topic?.locked ?? null,
              unread: topic?.unread ?? null,
              unreadCount: topic?.unreadCount ?? topic?.unread_count ?? null,
              msgCount: topic?.msgCount ?? topic?.msg_count ?? null,
              updatedAt: topic?.updatedAt ?? topic?.updated_at ?? null,
              lastMessageUpdatedAt:
                topic?.lastMessageUpdatedAt ??
                topic?.last_message_updated_at ??
                null,
              configHash: topic?.configHash ?? topic?.config_hash ?? null,
              contentHash: topic?.contentHash ?? topic?.content_hash ?? null,
            }))
            .sort((left, right) => left.id.localeCompare(right.id)),
        ),
      ),
      messageCount: list.reduce(
        (sum, topic) =>
          sum + (Number.isSafeInteger(topic?.msgCount) ? topic.msgCount : 0),
        0,
      ),
      selected: messageSnapshots,
    });
  }
  return {
    owners: ownerSnapshots,
    topicCount: ownerSnapshots.reduce(
      (sum, owner) => sum + owner.topicCount,
      0,
    ),
    messageCount: ownerSnapshots.reduce(
      (sum, owner) => sum + owner.messageCount,
      0,
    ),
  };
}

function processPid() {
  const output = runAdb(["shell", "pidof", "-s", DEBUG_PACKAGE], {
    allowFailure: true,
  }).trim();
  const pid = Number(output.split(/\s+/)[0]);
  return Number.isSafeInteger(pid) && pid > 0 ? pid : null;
}

async function waitForProcess(timeoutMs = 20_000) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const pid = processPid();
    if (pid) return pid;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("Android Debug 应用在限定时间内未恢复");
}

function codedError(code, message, cause) {
  const error = new Error(message, cause ? { cause } : undefined);
  error.code = code;
  return error;
}

async function reconnectAndroidCdp(timeoutMs = 30_000) {
  const startedAt = Date.now();
  let lastError = null;
  while (Date.now() - startedAt < timeoutMs) {
    let candidate = null;
    try {
      candidate = await connectAndroidCdp();
      await candidate.cdp.installEventBuffer();
      return candidate;
    } catch (error) {
      lastError = error;
      await candidate?.close().catch(() => {});
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw codedError(
    "ANDROID_CDP_RECONNECT_TIMEOUT",
    "Android WebView CDP 在限定时间内未就绪",
    lastError,
  );
}

function parsePssBytes(output) {
  const line = output
    .split(/\r?\n/)
    .find((item) => /^\s*TOTAL PSS:/i.test(item));
  const match = line?.match(/^\s*TOTAL PSS:\s*(\d+)/i);
  return match ? Number(match[1]) * 1024 : null;
}

function sampleAndroidMemory() {
  return parsePssBytes(
    runAdb(["shell", "dumpsys", "meminfo", DEBUG_PACKAGE], {
      allowFailure: true,
    }),
  );
}

function isForeground() {
  const displays = runAdb(["shell", "dumpsys", "window", "displays"], {
    allowFailure: true,
  });
  const output = displays.includes("mCurrentFocus=")
    ? displays
    : runAdb(["shell", "dumpsys", "window", "windows"], { allowFailure: true });
  return output
    .split(/\r?\n/)
    .some(
      (line) => line.includes("mCurrentFocus=") && line.includes(DEBUG_PACKAGE),
    );
}

async function waitForForeground(expected, timeoutMs = 10_000) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (isForeground() === expected) return true;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  return false;
}

async function backgroundAndResume() {
  runAdb(["shell", "input", "keyevent", "KEYCODE_HOME"]);
  const backgrounded = await waitForForeground(false);
  runAdb(["shell", "monkey", "-p", DEBUG_PACKAGE, "1"], {
    allowFailure: false,
  });
  await waitForProcess();
  const foregrounded = await waitForForeground(true);
  return { backgrounded, foregrounded };
}

async function forceStopAndReconnect(runtime) {
  const previous = runtime.cdp;
  const previousPid = previous?.pid || processPid();
  runtime.cdp = null;
  if (previous) await previous.close().catch(() => {});
  runAdb(["shell", "am", "force-stop", DEBUG_PACKAGE]);
  const stopped = processPid() === null;
  runAdb(["shell", "monkey", "-p", DEBUG_PACKAGE, "1"]);
  const nextPid = await waitForProcess().catch((cause) => {
    throw codedError(
      "ANDROID_PROCESS_RESTART_TIMEOUT",
      "Android Debug 应用在限定时间内未恢复",
      cause,
    );
  });
  const foregrounded = await waitForForeground(true);
  const next = await reconnectAndroidCdp();
  runtime.cdp = next;
  return {
    processRestarted: stopped && Boolean(nextPid) && nextPid !== previousPid,
    foregrounded,
  };
}

async function stopSync(cdp) {
  await cdp.invoke("stop_sync");
  for (let index = 0; index < 20; index += 1) {
    const active = await cdp.invoke("is_sync_active").catch(() => false);
    if (!active) return true;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  return false;
}

function appendMessage(cdp, ownerType, ownerId, topicId, message) {
  return cdp.invoke("append_single_message", {
    ownerId,
    ownerType,
    topicId,
    message,
  });
}

function deleteMessages(cdp, ownerType, ownerId, topicId, msgIds) {
  return cdp.invoke("delete_messages", {
    ownerId,
    ownerType,
    topicId,
    msgIds,
  });
}

function deleteTopic(cdp, ownerType, ownerId, topicId) {
  return cdp.invoke("delete_topic", {
    ownerId,
    ownerType,
    topicId,
  });
}

function saveAvatar(cdp, ownerType, ownerId, mimeType, bytes) {
  return cdp.invoke("save_avatar_data", {
    ownerType,
    ownerId,
    mimeType,
    imageData: [...bytes],
  });
}

module.exports = {
  compactHash,
  digest,
  loadTopicMessages,
  messageDigest,
  messageAttachmentSummary,
  snapshotTopics,
  sampleAndroidMemory,
  isForeground,
  backgroundAndResume,
  reconnectAndroidCdp,
  forceStopAndReconnect,
  stopSync,
  appendMessage,
  deleteMessages,
  deleteTopic,
  saveAvatar,
};
