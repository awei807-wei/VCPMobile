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
    const countToDelete = deps.currentChatHistory.value.length - targetIndex;
    await invoke("truncate_history_after_timestamp", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      timestamp: targetMessage.timestamp - 1,
    });
    if (!deps.isCurrentIdentity(identity)) return;
    deps.currentChatHistory.value.splice(targetIndex);
    deps.topicStore.decrementTopicMsgCount(identity, countToDelete);
    return;
  }

  await invoke("delete_messages", {
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
    msgIds: [messageId],
  });
  if (!deps.isCurrentIdentity(identity)) return;
  deps.currentChatHistory.value.splice(targetIndex, 1);
  deps.topicStore.decrementTopicMsgCount(identity, 1);
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
  clearMessageCache(messageId);
  const targetIndex = deps.currentChatHistory.value.findIndex(
    (message) => message.id === messageId,
  );
  if (targetIndex === -1) return;
  const message = deps.currentChatHistory.value[targetIndex];
  deps.currentChatHistory.value[targetIndex] = {
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
        ...deps.currentChatHistory.value[targetIndex],
        blocks: undefined,
      },
    });
    if (!deps.isCurrentIdentity(identity)) return;
    deps.currentChatHistory.value[targetIndex] = {
      ...deps.currentChatHistory.value[targetIndex],
      blocks: compiledBlocks as any,
    };
  } catch (error) {
    console.error("[updateMessageContent] patch_single_message failed:", error);
    if (!deps.isCurrentIdentity(identity)) return;
    deps.currentChatHistory.value[targetIndex] = {
      ...deps.currentChatHistory.value[targetIndex],
      blocks: [{ type: "markdown", content: newContent }],
    };
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
  message.blocks = blocks;
  try {
    await invoke("patch_single_message", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      message,
    });
  } catch (error) {
    console.error(
      `[ChatHistoryStore] Failed to persist message blocks for ${messageId}:`,
      error,
    );
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
  clearMessageCache(messageId);
  try {
    const compiledBlocks = await invoke<ContentBlock[]>("re_render_message", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      messageId,
      topicId,
    });
    if (!deps.isCurrentIdentity(identity)) return;
    deps.currentChatHistory.value[targetIndex] = {
      ...deps.currentChatHistory.value[targetIndex],
      blocks: compiledBlocks,
    };
  } catch (error) {
    console.error("[reRenderMessage] re_render_message failed:", error);
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
