"use strict";

const fs = require("node:fs/promises");
const path = require("node:path");
const {
  OWNER_A,
  SHARED_TOPIC,
  SHARED_MESSAGE,
  ATTACHMENT_HASH,
  buildFixtureModel,
} = require("./fixture-data.cjs");

async function writeJson(filePath, value) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function ownerConfig(owner) {
  if (owner.ownerType === "group") {
    return {
      id: owner.ownerId,
      name: `Synthetic group ${owner.ownerId}`,
      members: [],
      mode: "sequential",
      memberTags: {},
      groupPrompt: "Synthetic fixture; no user data.",
      invitePrompt: "",
      useUnifiedModel: false,
      unifiedModel: "",
      tagMatchMode: "strict",
      createdAt: 1700000000000,
      topics: owner.topics,
    };
  }
  return {
    name: `Synthetic ${owner.ownerType} ${owner.ownerId}`,
    systemPrompt: "Synthetic fixture; no user data.",
    model: "wire14-test-model",
    temperature: 0.5,
    contextTokenLimit: 4096,
    maxOutputTokens: 1024,
    streamOutput: true,
    topics: owner.topics,
  };
}

function historyPath(appDataPath, _ownerType, ownerId, topicId) {
  return path.join(
    appDataPath,
    "UserData",
    ownerId,
    "topics",
    topicId,
    "history.json",
  );
}

async function writeOwnerFixture(appDataPath, owner, histories) {
  const ownerRoot = owner.ownerType === "group" ? "AgentGroups" : "Agents";
  const ownerConfigPath = path.join(
    appDataPath,
    ownerRoot,
    owner.ownerId,
    "config.json",
  );
  await writeJson(ownerConfigPath, ownerConfig(owner));
  for (const topic of owner.topics) {
    const history = histories.get(`${owner.ownerId}\0${topic.id}`) || [];
    await writeJson(
      historyPath(appDataPath, owner.ownerType, owner.ownerId, topic.id),
      history,
    );
  }
}

async function writeAssets(appDataPath, model) {
  const avatarPath = path.join(
    appDataPath,
    "Agents",
    model.ids.ownerA,
    "avatar.png",
  );
  await fs.mkdir(path.dirname(avatarPath), { recursive: true });
  await fs.writeFile(avatarPath, model.assets.avatarBytes);
  const groupAvatarPath = path.join(
    appDataPath,
    "AgentGroups",
    model.ids.groupOwner,
    "avatar.png",
  );
  await fs.mkdir(path.dirname(groupAvatarPath), { recursive: true });
  await fs.writeFile(groupAvatarPath, model.assets.avatarBytes);
  const attachmentPath = path.join(
    appDataPath,
    "UserData",
    "attachments",
    `${model.assets.attachmentHash}.txt`,
  );
  await fs.mkdir(path.dirname(attachmentPath), { recursive: true });
  await fs.writeFile(attachmentPath, model.assets.attachmentBytes);
}

async function createScenarioFixture(appDataPath, options = {}) {
  const model = buildFixtureModel(options);
  await fs.mkdir(path.join(appDataPath, "AgentGroups"), { recursive: true });
  await fs.mkdir(path.join(appDataPath, "UserData", "attachments"), {
    recursive: true,
  });
  for (const owner of model.owners) {
    await writeOwnerFixture(appDataPath, owner, model.histories);
  }
  await writeAssets(appDataPath, model);
  return {
    ...model,
    paths: {
      history(ownerType, ownerId, topicId) {
        return historyPath(appDataPath, ownerType, ownerId, topicId);
      },
      ownerConfig(ownerType, ownerId) {
        const ownerRoot = ownerType === "group" ? "AgentGroups" : "Agents";
        return path.join(appDataPath, ownerRoot, ownerId, "config.json");
      },
      avatar(ownerType, ownerId) {
        const ownerRoot = ownerType === "group" ? "AgentGroups" : "Agents";
        return path.join(appDataPath, ownerRoot, ownerId, "avatar.png");
      },
      attachment(hash = ATTACHMENT_HASH) {
        return path.join(appDataPath, "UserData", "attachments", `${hash}.txt`);
      },
    },
  };
}

async function createFixture(appDataPath) {
  const model = await createScenarioFixture(appDataPath, { scaleTopics: 0 });
  return {
    ownerType: "agent",
    ownerId: OWNER_A,
    topicId: SHARED_TOPIC,
    messageId: SHARED_MESSAGE,
    model,
  };
}

module.exports = {
  ...require("./fixture-data.cjs"),
  createFixture,
  createScenarioFixture,
};
