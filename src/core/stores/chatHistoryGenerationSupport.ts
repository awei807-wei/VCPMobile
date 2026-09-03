import { Channel, invoke } from "@tauri-apps/api/core";
import type { ChatMessage, ContentBlock } from "../types/chat";
import {
  makeConversationIdentity,
  sameConversationIdentity,
  type ConversationIdentity,
  type ConversationOwnerType,
} from "./chatStoreIdentity";
import type {
  HistoryGenerationDeps,
  PendingGenerationOptions,
} from "./chatHistoryGenerationTypes";

export type GenerationContext = {
  identity: ConversationIdentity;
  pendingIdentity: ConversationIdentity;
  requestId: string;
};

export async function summarizeTopic(
  deps: HistoryGenerationDeps,
): Promise<void> {
  const identity = deps.currentIdentity();
  if (!identity) return;
  const topic = deps.topicStore.topics.find(
    (item: any) => item.id === identity.topicId,
  );
  const isDefaultName =
    topic && /^(新话题|新会话) \d{2}:\d{2}:\d{2}$/.test(topic.name);
  const messageCount = deps.currentChatHistory.value.filter(
    (message) => message.role !== "system",
  ).length;
  if (!isDefaultName || messageCount < 4) return;

  try {
    const agentName =
      deps.assistantStore.agents.find(
        (agent: any) => agent.id === identity.ownerId,
      )?.name || "AI";
    const newTitle = await invoke<string>("summarize_topic", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      agentName,
    });
    if (newTitle && deps.isCurrentIdentity(identity)) {
      await deps.topicStore.updateTopicTitle(
        identity.ownerId,
        identity.ownerType,
        identity.topicId,
        newTitle,
      );
    }
  } catch (error) {
    console.error("[ChatHistoryStore] AI Summary failed:", error);
  }
}

export function prepareGeneration(
  deps: HistoryGenerationDeps,
  userMsg: ChatMessage,
  pendingOptions: PendingGenerationOptions,
): GenerationContext | null {
  const selectedItem = deps.sessionStore.currentSelectedItem;
  const currentTopicId = deps.sessionStore.currentTopicId;
  const pendingIdentity = makeConversationIdentity(
    pendingOptions.ownerId || selectedItem?.id,
    pendingOptions.ownerType || selectedItem?.type,
    pendingOptions.topicId || currentTopicId,
  );
  const requestId = pendingOptions.requestId || userMsg.id;
  if (!pendingIdentity) return null;

  if (!pendingOptions.registered) {
    deps.streamStore.addPendingGeneration(
      pendingIdentity.ownerId,
      pendingIdentity.ownerType,
      pendingIdentity.topicId,
      requestId,
    );
  }

  const identity = makeConversationIdentity(
    selectedItem?.id,
    selectedItem?.type,
    currentTopicId,
  );
  if (!identity || !sameConversationIdentity(identity, pendingIdentity)) {
    if (!pendingOptions.registered) {
      deps.streamStore.removePendingGeneration(
        pendingIdentity.ownerId,
        pendingIdentity.ownerType,
        pendingIdentity.topicId,
        requestId,
      );
    }
    return null;
  }

  return { identity, pendingIdentity, requestId };
}

function updateCompiledMessage(
  deps: HistoryGenerationDeps,
  identity: ConversationIdentity,
  userMsg: ChatMessage,
  compiledBlocks: ContentBlock[],
): void {
  const targetIndex = deps.currentChatHistory.value.findIndex(
    (message) => message.id === userMsg.id,
  );
  if (targetIndex !== -1 && deps.isCurrentIdentity(identity)) {
    deps.currentChatHistory.value[targetIndex] = {
      ...deps.currentChatHistory.value[targetIndex],
      blocks: compiledBlocks as any,
    };
  }
}

export async function appendUserMessage(
  deps: HistoryGenerationDeps,
  identity: ConversationIdentity,
  userMsg: ChatMessage,
): Promise<void> {
  const compiledBlocks = await invoke<ContentBlock[]>("append_single_message", {
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
    message: { ...userMsg, blocks: undefined },
  });
  updateCompiledMessage(deps, identity, userMsg, compiledBlocks);
}

function handleGenerationMessage(
  deps: HistoryGenerationDeps,
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

function handleGenerationFinished(
  deps: HistoryGenerationDeps,
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
    void summarizeTopic(deps);
  }
}

export function createGenerationChannel(
  deps: HistoryGenerationDeps,
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
        handleGenerationMessage(
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
        handleGenerationFinished(deps, identity, topicId, ownerId, ownerType),
    });
  return streamChannel;
}

export async function invokeChatGeneration(
  identity: ConversationIdentity,
  userMsg: ChatMessage,
  settings: any,
  streamChannel: Channel<any>,
): Promise<void> {
  const commonPayload = {
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
    userMessage: userMsg,
    vcpUrl: settings.vcpServerUrl || "",
    vcpApiKey: settings.vcpApiKey || "",
  };
  if (identity.ownerType === "group") {
    await invoke("handle_group_chat_message", {
      payload: { groupId: identity.ownerId, ...commonPayload },
      streamChannel,
    });
    return;
  }
  await invoke("handle_agent_chat_message", {
    payload: { agentId: identity.ownerId, ...commonPayload },
    streamChannel,
  });
}
