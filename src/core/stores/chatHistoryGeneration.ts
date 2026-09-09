import { invoke } from "@tauri-apps/api/core";
import { acquireScreenKeep } from "../composables/useScreenKeeper";
import type { ChatMessage, ContentBlock } from "../types/chat";
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

interface MessageMutationResult {
  msgCount: number;
  deletedIds: string[];
  blocks?: ContentBlock[];
}

interface ResolvedEditedMessage {
  originalId: string;
  targetMessage: ChatMessage;
}

function readMessageMutationResult(value: unknown): MessageMutationResult {
  if (!value || typeof value !== "object")
    throw new Error("后端未返回有效的消息 mutation 结果");
  const result = value as {
    msgCount?: unknown;
    deletedIds?: unknown;
    blocks?: unknown;
  };
  if (
    typeof result.msgCount !== "number" ||
    !Number.isSafeInteger(result.msgCount) ||
    result.msgCount < 0 ||
    !Array.isArray(result.deletedIds) ||
    result.deletedIds.some((messageId) => typeof messageId !== "string") ||
    (result.blocks !== undefined && !Array.isArray(result.blocks))
  )
    throw new Error("后端返回的消息 mutation 结果无效");
  return {
    msgCount: result.msgCount,
    deletedIds: result.deletedIds as string[],
    blocks: result.blocks as ContentBlock[] | undefined,
  };
}

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
    if (!pendingOptions.userMessagePersisted)
      await appendUserMessage(deps, identity, userMsg);
    try {
      const settings = deps.settingsStore.settings;
      if (!settings) throw new Error("应用尚未完成初始化");
      const streamChannel = createGenerationChannel(deps, identity);
      await invokeChatGeneration(identity, userMsg, settings, streamChannel);
    } catch (error) {
      console.error("[ChatHistoryStore] Generation failed:", error);
    }
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

function resolveEditedMessage(
  deps: HistoryGenerationDeps,
  content: string,
): ResolvedEditedMessage | null {
  const originalId = deps.editingOriginalMessageId.value;
  if (!originalId) return null;
  const targetMessage = deps.currentChatHistory.value.find(
    (message) => message.id === originalId,
  );
  if (!targetMessage) return null;
  return {
    originalId,
    targetMessage: {
      ...targetMessage,
      content,
      blocks: [{ type: "markdown", content }],
    },
  };
}

function applyCommittedEditMutation(
  deps: HistoryGenerationDeps,
  identity: ConversationIdentity,
  mutation: MessageMutationResult,
): void {
  for (const messageId of mutation.deletedIds) {
    deps.streamStore.cancelUnreadReceipt?.(identity, messageId);
  }
  if (!deps.isCurrentIdentity(identity)) return;
  const deleted = new Set(mutation.deletedIds);
  deps.currentChatHistory.value = deps.currentChatHistory.value.filter(
    (message) =>
      message.id === deps.editingOriginalMessageId.value ||
      !deleted.has(message.id),
  );
  deps.topicStore.setTopicMsgCount?.(identity, mutation.msgCount);
}

function applyPersistedEdit(
  deps: HistoryGenerationDeps,
  identity: ConversationIdentity,
  targetMessage: ChatMessage,
  blocks?: ContentBlock[],
): void {
  if (!deps.isCurrentIdentity(identity)) return;
  const currentIndex = deps.currentChatHistory.value.findIndex(
    (message) => message.id === targetMessage.id,
  );
  if (currentIndex !== -1)
    deps.currentChatHistory.value[currentIndex] = {
      ...targetMessage,
      blocks: blocks ?? targetMessage.blocks,
    };
  deps.editingOriginalMessageId.value = null;
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
  const resolved = resolveEditedMessage(deps, content);
  if (!resolved) return false;
  const { originalId, targetMessage } = resolved;
  deps.streamStore.addPendingGeneration(
    identity.ownerId,
    identity.ownerType,
    identity.topicId,
    originalId,
  );
  try {
    const mutation = readMessageMutationResult(
      await invoke("edit_message_and_truncate_history", {
        ownerId: identity.ownerId,
        ownerType: identity.ownerType,
        topicId: identity.topicId,
        anchorMessageId: targetMessage.id,
        message: { ...targetMessage, blocks: undefined },
      }),
    );
    applyCommittedEditMutation(deps, identity, mutation);
    if (!deps.isCurrentIdentity(identity)) return true;
    applyPersistedEdit(deps, identity, targetMessage, mutation.blocks);
    await generate(targetMessage, {
      requestId: originalId,
      registered: true,
      userMessagePersisted: true,
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
    });
  } catch (error) {
    throw error;
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
    const userMessage = createUserMessage(
      deps,
      content,
      userMsgId,
      now,
      stagedAttachments.length > 0 ? stagedAttachments : undefined,
    );
    if (deps.isCurrentIdentity(identity))
      deps.currentChatHistory.value.push(userMessage);
    deps.topicStore.incrementTopicMsgCount(identity);
    await generate(userMessage, {
      requestId: userMsgId,
      registered: true,
      continueForCapturedIdentity: true,
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
  // An edit intent is authoritative.  If its local target disappeared while
  // the editor was open, fail closed instead of silently creating a new user
  // message in the same conversation.
  if (deps.editingOriginalMessageId.value) {
    if (!(await resendEditedMessage(deps, content, identity, generate))) {
      throw new Error("编辑目标消息未在当前历史记录中找到，已拒绝发送新消息");
    }
    return;
  }
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
