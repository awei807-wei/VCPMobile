import { invoke } from "@tauri-apps/api/core";
import type { Ref } from "vue";
import { clearMessageCache } from "../utils/astRenderer";
import type { ChatMessage, ContentBlock } from "../types/chat";
import type { ConversationIdentity } from "./chatStoreIdentity";

interface HistoryMutationDeps {
  currentChatHistory: Ref<ChatMessage[]>;
  sessionStore: any;
  topicStore: any;
  currentIdentity: () => ConversationIdentity | null;
  isCurrentIdentity: (identity: ConversationIdentity) => boolean;
  cancelUnreadReceipt?: (
    identity: ConversationIdentity,
    messageId: string,
  ) => void;
}

interface MessageMutationResult {
  msgCount: number;
  deletedIds: string[];
}

function readMessageMutationResult(value: unknown): MessageMutationResult {
  if (typeof value === "number") {
    if (!Number.isSafeInteger(value) || value < 0)
      throw new Error("后端返回的消息计数无效");
    return { msgCount: value, deletedIds: [] };
  }
  if (!value || typeof value !== "object")
    throw new Error("后端未返回有效的消息 mutation 结果");
  const result = value as {
    msgCount?: unknown;
    deletedIds?: unknown;
  };
  if (
    typeof result.msgCount !== "number" ||
    !Number.isSafeInteger(result.msgCount) ||
    result.msgCount < 0
  )
    throw new Error("后端返回的消息计数无效");
  const deletedIds = result.deletedIds ?? [];
  if (
    !Array.isArray(deletedIds) ||
    deletedIds.some((messageId) => typeof messageId !== "string")
  )
    throw new Error("后端返回的删除消息身份无效");
  return { msgCount: result.msgCount, deletedIds: deletedIds as string[] };
}

function applyAuthoritativeMessageCount(
  deps: HistoryMutationDeps,
  identity: ConversationIdentity,
  msgCount: number,
): void {
  deps.topicStore.setTopicMsgCount?.(identity, msgCount);
}

function removeMessagesById(
  deps: HistoryMutationDeps,
  identity: ConversationIdentity,
  messageIds: string[],
): void {
  if (!deps.isCurrentIdentity(identity) || messageIds.length === 0) return;
  const deleted = new Set(messageIds);
  deps.currentChatHistory.value = deps.currentChatHistory.value.filter(
    (message) => !deleted.has(message.id),
  );
}

async function deleteMessage(
  deps: HistoryMutationDeps,
  messageId: string,
  deleteAfter = false,
): Promise<void> {
  const identity = deps.currentIdentity();
  if (!identity) return;
  const targetIndex = deps.currentChatHistory.value.findIndex(
    (message) => message.id === messageId,
  );
  if (targetIndex === -1) return;
  const targetMessage = deps.currentChatHistory.value[targetIndex];
  if (deleteAfter) {
    const result = readMessageMutationResult(
      await invoke("truncate_history_after_timestamp", {
        ownerId: identity.ownerId,
        ownerType: identity.ownerType,
        topicId: identity.topicId,
        anchorMessageId: targetMessage.id,
        includeAnchor: true,
      }),
    );
    for (const deletedId of new Set([messageId, ...result.deletedIds])) {
      deps.cancelUnreadReceipt?.(identity, deletedId);
    }
    if (!deps.isCurrentIdentity(identity)) return;
    removeMessagesById(deps, identity, result.deletedIds);
    applyAuthoritativeMessageCount(deps, identity, result.msgCount);
    return;
  }

  const result = readMessageMutationResult(
    await invoke("delete_messages", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      msgIds: [messageId],
    }),
  );
  deps.cancelUnreadReceipt?.(identity, messageId);
  if (!deps.isCurrentIdentity(identity)) return;
  removeMessagesById(deps, identity, result.deletedIds);
  applyAuthoritativeMessageCount(deps, identity, result.msgCount);
}

async function deleteAttachment(
  deps: HistoryMutationDeps,
  topicId: string,
  messageId: string,
  hash: string,
): Promise<void> {
  const identity = deps.currentIdentity();
  if (!identity || identity.topicId !== topicId) return;
  await invoke("delete_message_attachment", {
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId,
    messageId,
    hash,
  });
  if (!deps.isCurrentIdentity(identity)) return;
  const target = deps.currentChatHistory.value.find(
    (message) => message.id === messageId,
  );
  if (target?.attachments)
    target.attachments = target.attachments.filter(
      (attachment) => attachment.hash !== hash,
    );
}

async function updateMessageContent(
  deps: HistoryMutationDeps,
  messageId: string,
  newContent: string,
): Promise<void> {
  const identity = deps.currentIdentity();
  if (!identity) return;
  const targetIndex = deps.currentChatHistory.value.findIndex(
    (message) => message.id === messageId,
  );
  if (targetIndex === -1) return;
  const message = deps.currentChatHistory.value[targetIndex];
  const nextMessage = {
    ...message,
    content: newContent,
    blocks: [{ type: "markdown", content: newContent }],
  };
  try {
    const compiledBlocks = await invoke("patch_single_message", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      message: {
        ...nextMessage,
        blocks: undefined,
      },
    });
    if (!deps.isCurrentIdentity(identity)) return;
    const currentIndex = deps.currentChatHistory.value.findIndex(
      (item) => item.id === messageId,
    );
    if (currentIndex === -1) return;
    clearMessageCache(messageId);
    deps.currentChatHistory.value[currentIndex] = {
      ...nextMessage,
      blocks: compiledBlocks as any,
    };
  } catch (error) {
    console.error("[updateMessageContent] patch_single_message 失败：", error);
    throw error;
  }
}

async function fetchRawContent(
  deps: HistoryMutationDeps,
  messageId: string,
): Promise<string> {
  const identity = deps.currentIdentity();
  if (!identity) return "";
  const existing = deps.currentChatHistory.value.find(
    (message) => message.id === messageId,
  );
  if (existing?.content) return existing.content;
  try {
    const content = await invoke<string>("fetch_raw_message_content", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      messageId,
    });
    if (existing && deps.isCurrentIdentity(identity))
      existing.content = content;
    return content;
  } catch {
    return "";
  }
}

async function persistMessageBlocks(
  deps: HistoryMutationDeps,
  messageId: string,
  blocks: ContentBlock[],
): Promise<void> {
  const identity = deps.currentIdentity();
  const message = deps.currentChatHistory.value.find(
    (item) => item.id === messageId,
  );
  if (!message || !identity) return;
  const nextMessage = { ...message, blocks };
  try {
    await invoke("patch_single_message", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      message: nextMessage,
    });
    if (!deps.isCurrentIdentity(identity)) return;
    const currentIndex = deps.currentChatHistory.value.findIndex(
      (item) => item.id === messageId,
    );
    if (currentIndex !== -1)
      deps.currentChatHistory.value[currentIndex] = nextMessage;
  } catch (error) {
    console.error(`[ChatHistoryStore] 保存消息块失败 ${messageId}：`, error);
    throw error;
  }
}

async function reRenderMessage(
  deps: HistoryMutationDeps,
  messageId: string,
  topicId: string,
): Promise<void> {
  const identity = deps.currentIdentity();
  if (!identity || identity.topicId !== topicId)
    throw new Error("消息所属会话已切换");
  const targetIndex = deps.currentChatHistory.value.findIndex(
    (message) => message.id === messageId,
  );
  if (targetIndex === -1) throw new Error("消息未在当前历史记录中找到");
  try {
    const compiledBlocks = await invoke<ContentBlock[]>("re_render_message", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      messageId,
      topicId,
    });
    if (!deps.isCurrentIdentity(identity)) return;
    const currentIndex = deps.currentChatHistory.value.findIndex(
      (item) => item.id === messageId,
    );
    if (currentIndex === -1) return;
    clearMessageCache(messageId);
    deps.currentChatHistory.value[currentIndex] = {
      ...deps.currentChatHistory.value[currentIndex],
      blocks: compiledBlocks,
    };
  } catch (error) {
    console.error("[reRenderMessage] re_render_message 失败：", error);
    throw error;
  }
}

export function createHistoryMutations(deps: HistoryMutationDeps) {
  return {
    deleteMessage: (messageId: string, deleteAfter = false) =>
      deleteMessage(deps, messageId, deleteAfter),
    deleteAttachment: (topicId: string, messageId: string, hash: string) =>
      deleteAttachment(deps, topicId, messageId, hash),
    updateMessageContent: (messageId: string, newContent: string) =>
      updateMessageContent(deps, messageId, newContent),
    fetchRawContent: (messageId: string) => fetchRawContent(deps, messageId),
    persistMessageBlocks: (messageId: string, blocks: ContentBlock[]) =>
      persistMessageBlocks(deps, messageId, blocks),
    reRenderMessage: (messageId: string, topicId: string) =>
      reRenderMessage(deps, messageId, topicId),
  };
}
