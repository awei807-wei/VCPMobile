import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { createStreamControls } from "./chatStreamControls";
import {
  cancelUnreadReceipt,
  cancelUnreadReceiptsForTopic,
  clearStreamMessageRendering,
  createStreamEventProcessor,
} from "./chatStreamProcessor";
import {
  nextStreamGeneration,
  type UnreadReceiptActiveGuard,
  type UnreadReceiptSubmission,
} from "./chatStreamProcessorSupport";
import {
  isStreamDebugEnabled,
  recordStreamTrace,
  streamDebugLog,
} from "./chatStreamDiagnostics";
import { createStreamShellFactory } from "./chatStreamPresentation";
import type { ChatMessage } from "../types/chat";
import type {
  ConversationIdentity,
  ConversationOwnerType,
} from "./chatStoreIdentity";
import type { ChatStreamStoreState } from "./chatStreamStoreState";
import type { ChatStreamIdentityApi } from "./chatStreamStoreIdentity";
import type { ChatStreamActivityApi } from "./chatStreamStoreActivity";

interface RuntimeDeps {
  state: ChatStreamStoreState;
  identity: ChatStreamIdentityApi;
  activity: ChatStreamActivityApi;
  assistantStore: Parameters<typeof createStreamShellFactory>[0];
  avatarStore: Parameters<typeof createStreamShellFactory>[1];
  topicStore: {
    incrementTopicMsgCount: (identity: ConversationIdentity) => void;
    incrementTopicUnreadCount: (
      identity: ConversationIdentity,
      messageId: string,
      isActive?: UnreadReceiptActiveGuard,
    ) => UnreadReceiptSubmission;
  };
}

export interface ChatStreamRuntimeApi {
  computeShell: ReturnType<typeof createStreamShellFactory>;
  processStreamEvent: ReturnType<typeof createStreamEventProcessor>;
  getActiveStreamMessage: (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ) => ChatMessage | undefined;
  getCurrentActiveStreamMessage: (messageId: string) => ChatMessage | undefined;
  allocateStreamGeneration: (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ) => number;
  stopMessage: ReturnType<typeof createStreamControls>["stopMessage"];
  stopGroupTurn: ReturnType<typeof createStreamControls>["stopGroupTurn"];
  cancelUnreadReceipt: (
    identity: ConversationIdentity,
    messageId: string,
  ) => void;
  cancelUnreadReceiptsForTopic: (identity: ConversationIdentity) => void;
}

function getActiveStreamMessage(
  deps: RuntimeDeps,
  ownerId: string,
  ownerType: ConversationOwnerType,
  topicId: string,
  messageId: string,
): ChatMessage | undefined {
  const identity = deps.identity.explicitIdentity(ownerId, ownerType, topicId);
  const key = identity && deps.identity.activeMessageKey(identity, messageId);
  return key ? deps.state.activeStreamMessages.get(key) : undefined;
}

function createProcessor(deps: RuntimeDeps) {
  const computeShell = createStreamShellFactory(
    deps.assistantStore,
    deps.avatarStore,
  );
  const processStreamEvent = createStreamEventProcessor({
    state: deps.state.runtimeState,
    computeShell,
    addSessionStream: deps.activity.addSessionStream,
    removeSessionStream: deps.activity.removeSessionStream,
    incrementTopicMsgCount: (identity) =>
      deps.topicStore.incrementTopicMsgCount(identity),
    incrementTopicUnreadCount: (identity, messageId) =>
      deps.topicStore.incrementTopicUnreadCount(identity, messageId),
    incrementTopicUnreadCountWithGuard: (identity, messageId, isActive) =>
      deps.topicStore.incrementTopicUnreadCount(identity, messageId, isActive),
    currentIdentity: deps.identity.currentIdentity,
    invoke: (command, args) => tauriInvoke(command, args),
    isStreamDebugEnabled,
    recordStreamTrace,
    streamDebugLog,
  });
  return { computeShell, processStreamEvent };
}

function createMessageGetters(
  deps: RuntimeDeps,
  getActive: ChatStreamRuntimeApi["getActiveStreamMessage"],
) {
  const getCurrentActiveStreamMessage = (messageId: string) => {
    const identity = deps.identity.currentIdentity();
    return identity
      ? getActive(
          identity.ownerId,
          identity.ownerType,
          identity.topicId,
          messageId,
        )
      : undefined;
  };
  return { getCurrentActiveStreamMessage };
}

function allocateStreamGeneration(
  deps: RuntimeDeps,
  ownerId: string,
  ownerType: ConversationOwnerType,
  topicId: string,
  messageId: string,
): number {
  const identity = deps.identity.explicitIdentity(ownerId, ownerType, topicId);
  const key = identity && deps.identity.activeMessageKey(identity, messageId);
  if (!key) return 0;
  return nextStreamGeneration(deps.state.runtimeState, key);
}

export function createChatStreamRuntime(
  deps: RuntimeDeps,
): ChatStreamRuntimeApi {
  const processor = createProcessor(deps);
  const getActive = (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ) => getActiveStreamMessage(deps, ownerId, ownerType, topicId, messageId);
  const getters = createMessageGetters(deps, getActive);
  const controls = createStreamControls({
    currentIdentity: deps.identity.currentIdentity,
    activeStreamingIds: deps.activity.activeStreamingIds,
    activeStreamMessages: deps.state.activeStreamMessages,
    runtimeState: deps.state.runtimeState,
    streamingMessageId: deps.state.streamingMessageId,
    streamingMessageKey: deps.state.streamingMessageKey,
    removeSessionStream: deps.activity.removeSessionStream,
    activeMessageKey: deps.identity.activeMessageKey,
  });
  return {
    ...processor,
    ...getters,
    getActiveStreamMessage: getActive,
    allocateStreamGeneration: (
      ownerId: string,
      ownerType: ConversationOwnerType,
      topicId: string,
      messageId: string,
    ) =>
      allocateStreamGeneration(
        deps,
        ownerId,
        ownerType,
        topicId,
        messageId,
      ),
    stopMessage: controls.stopMessage,
    stopGroupTurn: controls.stopGroupTurn,
    cancelUnreadReceipt: (identity, messageId) =>
      cancelUnreadReceipt(deps.state.runtimeState, identity, messageId),
    cancelUnreadReceiptsForTopic: (identity) =>
      cancelUnreadReceiptsForTopic(deps.state.runtimeState, identity),
  };
}

export { clearStreamMessageRendering };
