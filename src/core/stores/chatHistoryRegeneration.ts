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
}

function handleRegenerationMessage(
  deps: RegenerationDeps,
  identity: ConversationIdentity,
  message: ChatMessage,
  topicId: string,
  ownerId: string,
  ownerType: ConversationOwnerType,
): void {
  const eventIdentity = makeConversationIdentity(ownerId, ownerType, topicId);
  if (
    eventIdentity &&
    sameConversationIdentity(eventIdentity, identity) &&
    deps.isCurrentIdentity(identity) &&
    !deps.currentChatHistory.value.some((item) => item.id === message.id)
  ) {
    deps.currentChatHistory.value.push(message);
    deps.currentChatHistory.value.sort(
      (left, right) => left.timestamp - right.timestamp,
    );
  }
}

function handleRegenerationFinished(
  deps: RegenerationDeps,
  identity: ConversationIdentity,
  topicId: string,
  ownerId: string,
  ownerType: ConversationOwnerType,
): void {
  const eventIdentity = makeConversationIdentity(ownerId, ownerType, topicId);
  if (
    eventIdentity &&
    sameConversationIdentity(eventIdentity, identity) &&
    deps.isCurrentIdentity(identity)
  ) {
    void deps.summarizeTopic();
  }
}

function createRegenerationChannel(
  deps: RegenerationDeps,
  identity: ConversationIdentity,
): Channel<any> {
  const streamChannel = new Channel<any>();
  streamChannel.onmessage = (event) =>
    deps.streamStore.processStreamEvent(event, {
      onMessageCreated: (
        message: ChatMessage,
        topicId: string,
        ownerId: string,
        ownerType: ConversationOwnerType,
      ) =>
        handleRegenerationMessage(
          deps,
          identity,
          message,
          topicId,
          ownerId,
          ownerType,
        ),
      onStreamFinished: (
        _messageId: string,
        topicId: string,
        ownerId: string,
        ownerType: ConversationOwnerType,
      ) =>
        handleRegenerationFinished(deps, identity, topicId, ownerId, ownerType),
    });
  return streamChannel;
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

  let lastUserMsgIndex = targetIndex - 1;
  while (
    lastUserMsgIndex >= 0 &&
    deps.currentChatHistory.value[lastUserMsgIndex].role !== "user"
  )
    lastUserMsgIndex -= 1;
  if (lastUserMsgIndex === -1) return;

  const lastUserMsg = deps.currentChatHistory.value[lastUserMsgIndex];
  const countToDelete =
    deps.currentChatHistory.value.length - lastUserMsgIndex - 1;
  if (deps.isCurrentIdentity(identity)) {
    deps.currentChatHistory.value = deps.currentChatHistory.value.slice(
      0,
      lastUserMsgIndex + 1,
    );
    deps.topicStore.decrementTopicMsgCount(identity, countToDelete);
  }

  acquireScreenKeep();
  const pendingRequestId = `regen_${lastUserMsg.id}`;
  deps.streamStore.addPendingGeneration(
    identity.ownerId,
    identity.ownerType,
    identity.topicId,
    pendingRequestId,
  );
  try {
    const streamChannel = createRegenerationChannel(deps, identity);
    await invoke("regenerate_topic_response", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      targetUserMsgId: lastUserMsg.id,
      streamChannel,
    });
  } catch (error) {
    console.error("[ChatHistoryStore] Regeneration failed:", error);
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
