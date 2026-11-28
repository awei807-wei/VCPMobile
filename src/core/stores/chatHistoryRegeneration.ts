import { invoke } from "@tauri-apps/api/core";
import { acquireScreenKeep } from "../composables/useScreenKeeper";
import type { Ref } from "vue";
import type { ChatMessage } from "../types/chat";
import type { PendingGenerationOptions } from "./chatHistoryGenerationTypes";
import type { ConversationIdentity } from "./chatStoreIdentity";

interface RegenerationDeps {
  currentChatHistory: Ref<ChatMessage[]>;
  streamStore: any;
  topicStore: any;
  currentIdentity: () => ConversationIdentity | null;
  isCurrentIdentity: (identity: ConversationIdentity) => boolean;
  /** 标准生成管道入口（与首次发送、编辑重发完全同构）。 */
  triggerGeneration: (
    message: ChatMessage,
    options: PendingGenerationOptions,
  ) => Promise<void>;
  reloadCurrentHistory?: (identity: ConversationIdentity) => Promise<void>;
}

interface RegenerationPreparation {
  msgCount: number;
  deletedIds: string[];
  userMessage: ChatMessage;
}

function readPreparationResult(value: unknown): RegenerationPreparation {
  if (!value || typeof value !== "object")
    throw new Error("后端未返回有效的重新生成准备结果");
  const result = value as {
    msgCount?: unknown;
    deletedIds?: unknown;
    userMessage?: unknown;
  };
  if (
    typeof result.msgCount !== "number" ||
    !Number.isSafeInteger(result.msgCount) ||
    result.msgCount < 0 ||
    !Array.isArray(result.deletedIds) ||
    result.deletedIds.some((messageId) => typeof messageId !== "string")
  )
    throw new Error("后端返回的重新生成准备结果无效");
  const userMessage = result.userMessage as ChatMessage | undefined;
  if (
    !userMessage ||
    typeof userMessage !== "object" ||
    typeof userMessage.id !== "string" ||
    typeof userMessage.role !== "string"
  )
    throw new Error("后端返回的重新生成锚点消息无效");
  return {
    msgCount: result.msgCount,
    deletedIds: result.deletedIds as string[],
    userMessage,
  };
}

/**
 * 两阶段重新生成：
 * 1. `prepare_topic_regeneration` 只做截断与旧请求取消，立即返回 deletedIds / msgCount / userMessage；
 * 2. 立刻淘汰旧气泡，再复用标准生成管道启动新一轮 SSE，思考态与流式吐字即时可见。
 *
 * 旧实现把整轮生成塞在同一个 IPC 里同步 await，前端拿不到 deletedIds，
 * 气泡会一直卡在已回复状态；这里彻底切断该耦合。
 */
async function regenerateResponse(
  deps: RegenerationDeps,
  targetMessageId: string,
): Promise<void> {
  const identity = deps.currentIdentity();
  const targetIndex = deps.currentChatHistory.value.findIndex(
    (message) => message.id === targetMessageId,
  );
  if (!identity || targetIndex === -1) return;

  acquireScreenKeep();
  const pendingRequestId = `regen_${targetMessageId}`;
  deps.streamStore.addPendingGeneration(
    identity.ownerId,
    identity.ownerType,
    identity.topicId,
    pendingRequestId,
  );
  try {
    const prepared = readPreparationResult(
      await invoke("prepare_topic_regeneration", {
        ownerId: identity.ownerId,
        ownerType: identity.ownerType,
        topicId: identity.topicId,
        targetResponseMsgId: targetMessageId,
      }),
    );
    for (const messageId of prepared.deletedIds) {
      deps.streamStore.cancelUnreadReceipt?.(identity, messageId);
    }
    if (!deps.isCurrentIdentity(identity)) return;
    const deleted = new Set(prepared.deletedIds);
    deps.currentChatHistory.value = deps.currentChatHistory.value.filter(
      (message) => !deleted.has(message.id),
    );
    deps.topicStore.setTopicMsgCount?.(identity, prepared.msgCount);
    await deps.triggerGeneration(prepared.userMessage, {
      requestId: pendingRequestId,
      registered: true,
      userMessagePersisted: true,
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
    });
  } catch (error) {
    console.error("[ChatHistoryStore] 重新生成失败：", error);
    if (deps.isCurrentIdentity(identity) && deps.reloadCurrentHistory) {
      try {
        await deps.reloadCurrentHistory(identity);
      } catch (reloadError) {
        console.error("[ChatHistoryStore] 重新加载历史失败：", reloadError);
      }
    }
  } finally {
    deps.streamStore.removePendingGeneration(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      pendingRequestId,
    );
  }
}

export function createHistoryRegeneration(deps: RegenerationDeps) {
  return (targetMessageId: string) => regenerateResponse(deps, targetMessageId);
}
