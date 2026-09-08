import { Channel, invoke } from "@tauri-apps/api/core";
import { acquireScreenKeep } from "../composables/useScreenKeeper";
import type { Ref } from "vue";
import type { ChatMessage } from "../types/chat";
import {
  makeConversationIdentity,
  sameConversationIdentity,
  type ConversationIdentity,
  type ConversationOwnerType,
} from "./chatStoreIdentity";

interface RegenerationDeps {
  currentChatHistory: Ref<ChatMessage[]>;
  sessionStore: any;
  streamStore: any;
  topicStore: any;
  currentIdentity: () => ConversationIdentity | null;
  isCurrentIdentity: (identity: ConversationIdentity) => boolean;
  summarizeTopic: () => Promise<void>;
  reloadCurrentHistory?: (identity: ConversationIdentity) => Promise<void>;
}

interface RegenerationMutationResult {
  msgCount: number;
  deletedIds: string[];
}

function readRegenerationResult(value: unknown): RegenerationMutationResult {
  if (!value || typeof value !== "object")
    throw new Error("后端未返回有效的重新生成结果");
  const result = value as { msgCount?: unknown; deletedIds?: unknown };
  if (
    typeof result.msgCount !== "number" ||
    !Number.isSafeInteger(result.msgCount) ||
    result.msgCount < 0 ||
    !Array.isArray(result.deletedIds) ||
    result.deletedIds.some((messageId) => typeof messageId !== "string")
  )
    throw new Error("后端返回的重新生成结果无效");
  return {
    msgCount: result.msgCount,
    deletedIds: result.deletedIds as string[],
  };
}

function handleRegenerationMessage(
  deps: RegenerationDeps,
  identity: ConversationIdentity,
  message: ChatMessage,
  topicId: string,
  ownerId: string,
  ownerType: ConversationOwnerType,
  generation: number,
): void {
  const eventIdentity = makeConversationIdentity(ownerId, ownerType, topicId);
  if (
    !eventIdentity ||
    !sameConversationIdentity(eventIdentity, identity) ||
    !deps.isCurrentIdentity(identity)
  )
    return;
  message.generation = generation;
  const targetIndex = deps.currentChatHistory.value.findIndex(
    (item) => item.id === message.id,
  );
  if (targetIndex === -1) deps.currentChatHistory.value.push(message);
  else {
    const existing = deps.currentChatHistory.value[targetIndex];
    if (existing.role !== message.role) return;
    if (existing.generation !== undefined && existing.generation > generation)
      return;
    if (existing !== message)
      deps.currentChatHistory.value[targetIndex] = message;
  }
  deps.currentChatHistory.value.sort(
    (left, right) => left.timestamp - right.timestamp,
  );
}

function handleRegenerationFinished(
  deps: RegenerationDeps,
  identity: ConversationIdentity,
  topicId: string,
  ownerId: string,
  ownerType: ConversationOwnerType,
  messageId: string,
  generation: number,
): void {
  const eventIdentity = makeConversationIdentity(ownerId, ownerType, topicId);
  if (
    eventIdentity &&
    sameConversationIdentity(eventIdentity, identity) &&
    deps.isCurrentIdentity(identity) &&
    deps.currentChatHistory.value.some(
      (message) =>
        message.id === messageId && message.generation === generation,
    )
  ) {
    void deps.summarizeTopic();
  }
}

function createRegenerationChannel(
  deps: RegenerationDeps,
  identity: ConversationIdentity,
): { channel: Channel<any>; commit: () => void } {
  const streamChannel = new Channel<any>();
  const bufferedEvents: any[] = [];
  let committed = false;
  const processEvent = (event: any) =>
    deps.streamStore.processStreamEvent(event, {
      onMessageCreated: (
        message: ChatMessage,
        topicId: string,
        ownerId: string,
        ownerType: ConversationOwnerType,
        generation: number,
      ) =>
        handleRegenerationMessage(
          deps,
          identity,
          message,
          topicId,
          ownerId,
          ownerType,
          generation,
        ),
      onStreamFinished: (
        messageId: string,
        topicId: string,
        ownerId: string,
        ownerType: ConversationOwnerType,
        generation: number,
      ) =>
        handleRegenerationFinished(
          deps,
          identity,
          topicId,
          ownerId,
          ownerType,
          messageId,
          generation,
        ),
    });
  streamChannel.onmessage = (event) => {
    if (!committed) {
      bufferedEvents.push(event);
      return;
    }
    processEvent(event);
  };
  return {
    channel: streamChannel,
    commit: () => {
      committed = true;
      const pendingEvents = bufferedEvents;
      bufferedEvents.length = 0;
      pendingEvents.forEach(processEvent);
    },
  };
}

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
    const regenerationChannel = createRegenerationChannel(deps, identity);
    const result = readRegenerationResult(
      await invoke("regenerate_topic_response", {
        ownerId: identity.ownerId,
        ownerType: identity.ownerType,
        topicId: identity.topicId,
        targetResponseMsgId: targetMessageId,
        streamChannel: regenerationChannel.channel,
      }),
    );
    for (const messageId of result.deletedIds) {
      deps.streamStore.cancelUnreadReceipt?.(identity, messageId);
    }
    if (deps.isCurrentIdentity(identity)) {
      const deleted = new Set(result.deletedIds);
      deps.currentChatHistory.value = deps.currentChatHistory.value.filter(
        (message) => !deleted.has(message.id),
      );
      deps.topicStore.setTopicMsgCount?.(identity, result.msgCount);
    }
    regenerationChannel.commit();
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
