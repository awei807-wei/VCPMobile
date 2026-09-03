import { invoke } from "@tauri-apps/api/core";
import { acquireScreenKeep } from "../composables/useScreenKeeper";
import type { ChatMessage } from "../types/chat";
import { hasWorkingAttachments } from "./attachmentSendGate";
import type {
  HistoryGenerationDeps,
  PendingGenerationOptions,
} from "./chatHistoryGenerationTypes";
import {
  appendUserMessage,
  createGenerationChannel,
  invokeChatGeneration,
  prepareGeneration,
  summarizeTopic,
} from "./chatHistoryGenerationSupport";
import type { ConversationIdentity } from "./chatStoreIdentity";

async function triggerGeneration(
  deps: HistoryGenerationDeps,
  userMsg: ChatMessage,
  pendingOptions: PendingGenerationOptions = {},
): Promise<void> {
  const generation = prepareGeneration(deps, userMsg, pendingOptions);
  if (!generation) return;

  const { identity, pendingIdentity, requestId } = generation;
  acquireScreenKeep();
  try {
    await appendUserMessage(deps, identity, userMsg);
    const settings = deps.settingsStore.settings;
    if (!settings) throw new Error("应用尚未完成初始化");
    const streamChannel = createGenerationChannel(deps, identity);
    await invokeChatGeneration(identity, userMsg, settings, streamChannel);
  } catch (error) {
    console.error("[ChatHistoryStore] Generation failed:", error);
  } finally {
    deps.streamStore.removePendingGeneration(
      pendingIdentity.ownerId,
      pendingIdentity.ownerType,
      pendingIdentity.topicId,
      requestId,
    );
  }
}

function getSendIdentity(
  deps: HistoryGenerationDeps,
  content: string,
): ConversationIdentity | null {
  const identity = deps.currentIdentity();
  if (!identity) return null;
  if (hasWorkingAttachments(deps.attachmentStore.stagedAttachments))
    return null;
  if (!content.trim() && deps.attachmentStore.stagedAttachments.length === 0)
    return null;
  return identity;
}

async function resendEditedMessage(
  deps: HistoryGenerationDeps,
  content: string,
  identity: ConversationIdentity,
  generate: (
    message: ChatMessage,
    options: PendingGenerationOptions,
  ) => Promise<void>,
): Promise<boolean> {
  const originalId = deps.editingOriginalMessageId.value;
  if (!originalId) return false;
  deps.editingOriginalMessageId.value = null;
  const targetIndex = deps.currentChatHistory.value.findIndex(
    (message) => message.id === originalId,
  );
  if (targetIndex === -1) return false;

  const targetMessage = deps.currentChatHistory.value[targetIndex];
  deps.streamStore.addPendingGeneration(
    identity.ownerId,
    identity.ownerType,
    identity.topicId,
    originalId,
  );
  try {
    targetMessage.content = content;
    targetMessage.blocks = [{ type: "markdown", content }];
    await invoke("truncate_history_after_timestamp", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      timestamp: targetMessage.timestamp,
    });
    if (deps.isCurrentIdentity(identity)) {
      deps.currentChatHistory.value = deps.currentChatHistory.value.slice(
        0,
        targetIndex + 1,
      );
    }
    await generate(targetMessage, {
      requestId: originalId,
      registered: true,
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
    });
  } finally {
    deps.streamStore.removePendingGeneration(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      originalId,
    );
  }
  return true;
}

function createUserMessage(
  deps: HistoryGenerationDeps,
  content: string,
  userMsgId: string,
  timestamp: number,
  attachments: ChatMessage["attachments"],
): ChatMessage {
  const userName = deps.settingsStore.settings?.userName || "User";
  return {
    id: userMsgId,
    role: "user",
    name: userName,
    content,
    timestamp,
    attachments,
    shell: deps.streamStore.computeShell({ role: "user", name: userName }),
    blocks: [{ type: "markdown", content }],
  };
}

async function sendNewMessage(
  deps: HistoryGenerationDeps,
  content: string,
  identity: ConversationIdentity,
  generate: (
    message: ChatMessage,
    options: PendingGenerationOptions,
  ) => Promise<void>,
): Promise<void> {
  const now = Date.now();
  const userMsgId = `msg_${now}_user_${Math.random().toString(36).substring(2, 9)}`;
  deps.streamStore.addPendingGeneration(
    identity.ownerId,
    identity.ownerType,
    identity.topicId,
    userMsgId,
  );
  try {
    const stagedAttachments = [...deps.attachmentStore.stagedAttachments];
    if (hasWorkingAttachments(stagedAttachments)) return;
    deps.attachmentStore.clearStaged();
    if (stagedAttachments.length > 0)
      await deps.attachmentStore.preProcessDocuments(stagedAttachments);
    if (!deps.isCurrentIdentity(identity)) return;
    const userMessage = createUserMessage(
      deps,
      content,
      userMsgId,
      now,
      stagedAttachments.length > 0 ? stagedAttachments : undefined,
    );
    deps.currentChatHistory.value.push(userMessage);
    deps.topicStore.incrementTopicMsgCount(identity);
    await generate(userMessage, {
      requestId: userMsgId,
      registered: true,
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
    });
  } finally {
    deps.streamStore.removePendingGeneration(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      userMsgId,
    );
  }
}

async function sendMessage(
  deps: HistoryGenerationDeps,
  content: string,
): Promise<void> {
  if (deps.switchGuardStore.switching) return;
  const identity = getSendIdentity(deps, content);
  if (!identity) return;
  const generate = (message: ChatMessage, options: PendingGenerationOptions) =>
    triggerGeneration(deps, message, options);
  if (await resendEditedMessage(deps, content, identity, generate)) return;
  await sendNewMessage(deps, content, identity, generate);
}

export function createHistoryGeneration(deps: HistoryGenerationDeps) {
  return {
    summarizeTopic: () => summarizeTopic(deps),
    triggerGeneration: (
      userMsg: ChatMessage,
      pendingOptions: PendingGenerationOptions = {},
    ) => triggerGeneration(deps, userMsg, pendingOptions),
    sendMessage: (content: string) => sendMessage(deps, content),
  };
}
