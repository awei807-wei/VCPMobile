"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs/promises");

function digest(value) {
  return crypto
    .createHash("sha256")
    .update(JSON.stringify(value))
    .digest("hex");
}

function compactHash(value) {
  if (typeof value !== "string" || value.length === 0) return null;
  return `${value.slice(0, 8)}…${value.slice(-4)}`;
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

function messageHash(messages) {
  return compactHash(
    digest((Array.isArray(messages) ? messages : []).map(normalizedMessage)),
  );
}

async function readJson(filePath, fallback = null) {
  try {
    return JSON.parse(await fs.readFile(filePath, "utf8"));
  } catch (error) {
    if (error?.code === "ENOENT") return fallback;
    throw error;
  }
}

async function writeJson(filePath, value) {
  await fs.mkdir(require("node:path").dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

async function readHistory(runtime, ownerId, topicId, ownerType = "agent") {
  const filePath = runtime.fixture.paths.history(ownerType, ownerId, topicId);
  const history = await readJson(filePath, []);
  if (!Array.isArray(history))
    throw new Error("桌面合成 history.json 不是数组");
  return history;
}

async function appendHistoryMessage(
  runtime,
  ownerId,
  topicId,
  message,
  ownerType = "agent",
) {
  const history = await readHistory(runtime, ownerId, topicId, ownerType);
  if (history.some((item) => item?.id === message?.id)) {
    throw new Error("桌面合成 fixture 消息 ID 已存在");
  }
  history.push(message);
  history.sort(
    (left, right) => (left?.timestamp || 0) - (right?.timestamp || 0),
  );
  await writeJson(
    runtime.fixture.paths.history(ownerType, ownerId, topicId),
    history,
  );
  return history;
}

async function removeHistoryMessage(
  runtime,
  ownerId,
  topicId,
  messageId,
  ownerType = "agent",
) {
  const history = await readHistory(runtime, ownerId, topicId, ownerType);
  const next = history.filter((message) => message?.id !== messageId);
  await writeJson(
    runtime.fixture.paths.history(ownerType, ownerId, topicId),
    next,
  );
  return { removed: history.length - next.length, history: next };
}

async function readOwnerConfig(runtime, ownerId, ownerType = "agent") {
  if (ownerType !== "agent" && ownerType !== "group") {
    throw new Error("桌面合成 owner 类型无效");
  }
  const config = await readJson(
    runtime.fixture.paths.ownerConfig(ownerType, ownerId),
  );
  if (!config || typeof config !== "object" || !Array.isArray(config.topics)) {
    throw new Error("桌面合成 owner config 无效");
  }
  return config;
}

async function topicExists(runtime, ownerId, topicId, ownerType = "agent") {
  const config = await readOwnerConfig(runtime, ownerId, ownerType);
  return config.topics.some((topic) => topic?.id === topicId);
}

async function waitForCds(runtime, timeoutMs = 20_000) {
  const facade = runtime?.desktop?.facade || runtime?.facade;
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    try {
      await facade.client.health();
      return true;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  return false;
}

async function reconcile(runtime, ownerId, topicId, ownerType = "agent") {
  const client = runtime?.desktop?.facade?.client || runtime?.facade?.client;
  if (!client) {
    const error = new Error("隔离 CDS client 不可用");
    error.code = "DESKTOP_CDS_CLIENT_UNAVAILABLE";
    throw error;
  }
  const filePath = runtime.fixture.paths.history(ownerType, ownerId, topicId);
  let result;
  try {
    result = await client.ingestHistoryPath(filePath, "electron");
  } catch (cause) {
    const suffix =
      typeof cause?.code === "string" && /^[A-Z0-9_]{1,48}$/.test(cause.code)
        ? cause.code
        : "FAILED";
    const error = new Error("隔离 CDS history ingest 失败", { cause });
    error.code = `DESKTOP_INGEST_${suffix}`;
    throw error;
  }
  // 文件监听器与本次显式摄取共享同一把锁。监听器若先完成，CDS 会以
  // accepted:false 表示“路径合法但内容已是最新”，这同样是成功收敛。
  if (!result || typeof result.accepted !== "boolean") {
    const error = new Error("隔离 CDS history ingest 返回无效结果");
    error.code = "DESKTOP_INGEST_INVALID_RESPONSE";
    throw error;
  }
  return result;
}

function parseProcMemory(status) {
  const rss = status.match(/^VmRSS:\s+(\d+)\s+kB$/m)?.[1];
  const peak = status.match(/^VmHWM:\s+(\d+)\s+kB$/m)?.[1];
  return {
    rssBytes: rss ? Number(rss) * 1024 : null,
    peakRssBytes: peak ? Number(peak) * 1024 : null,
  };
}

async function sampleDesktopMemory(runtime) {
  const pid = runtime?.facade?.lifecycle?.process?.pid;
  if (!Number.isSafeInteger(pid) || pid <= 0)
    return { rssBytes: null, peakRssBytes: null };
  try {
    return parseProcMemory(await fs.readFile(`/proc/${pid}/status`, "utf8"));
  } catch {
    return { rssBytes: null, peakRssBytes: null };
  }
}

async function historyEvidence(runtime, ownerId, topicId, ownerType = "agent") {
  const history = await readHistory(runtime, ownerId, topicId, ownerType);
  const attachments = history.flatMap((message) =>
    Array.isArray(message?.attachments) ? message.attachments : [],
  );
  return {
    messageCount: history.length,
    messageHash: messageHash(history),
    attachmentCount: attachments.length,
    validAttachmentCount: attachments.filter(
      (attachment) =>
        typeof attachment?.hash === "string" &&
        /^[a-f0-9]{64}$/i.test(attachment.hash),
    ).length,
    attachmentHashes: [
      ...new Set(
        attachments
          .map((attachment) => attachment?.hash)
          .filter(
            (hash) => typeof hash === "string" && /^[a-f0-9]{64}$/i.test(hash),
          )
          .map(compactHash),
      ),
    ].slice(0, 8),
  };
}

async function snapshotFixtureState(runtime) {
  const owners = [];
  let topicCount = 0;
  let messageCount = 0;
  for (const owner of runtime.fixture.owners) {
    const config = await readOwnerConfig(
      runtime,
      owner.ownerId,
      owner.ownerType,
    );
    const histories = [];
    for (const topic of owner.topics) {
      const history = await readHistory(
        runtime,
        owner.ownerId,
        topic.id,
        owner.ownerType,
      );
      histories.push({
        topicId: topic.id,
        messageCount: history.length,
        messageHash: messageHash(history),
      });
      messageCount += history.length;
    }
    topicCount += owner.topics.length;
    owners.push({
      ownerType: owner.ownerType,
      ownerId: owner.ownerId,
      configHash: compactHash(digest(config)),
      histories,
    });
  }
  return {
    topicCount,
    messageCount,
    stateHash: compactHash(digest(owners)),
  };
}

module.exports = {
  digest,
  compactHash,
  readJson,
  writeJson,
  readHistory,
  appendHistoryMessage,
  removeHistoryMessage,
  readOwnerConfig,
  topicExists,
  waitForCds,
  reconcile,
  sampleDesktopMemory,
  historyEvidence,
  snapshotFixtureState,
};
