import { defineStore } from "pinia";
import { computed, onScopeDispose, reactive, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { releaseScreenKeep } from "../composables/useScreenKeeper";
import { useChatSessionStore } from "./chatSessionStore";
import { useAssistantStore } from "./assistant";
import { useAvatarStore } from "./avatar";
import { useTopicStore } from "./topicListManager";
import {
  currentConversationIdentity,
  makeConversationIdentity,
  makeMessageIdentity,
  messageIdentityKey,
  topicIdentityKey,
  type ConversationIdentity,
  type ConversationOwnerType,
} from "./chatStoreIdentity";
import {
  clearStreamMessageRendering,
  clearStreamRendering,
  createStreamEventProcessor,
  type StreamState,
} from "./chatStreamProcessor";
import { createStreamShellFactory } from "./chatStreamPresentation";
import {
  isStreamDebugEnabled,
  recordStreamTrace,
  streamDebugLog,
} from "./chatStreamDiagnostics";
import { createStreamControls } from "./chatStreamControls";
import type { ChatMessage } from "../types/chat";

export const useChatStreamStore = defineStore("chatStream", () => {
  const streamingMessageId = ref<string | null>(null);
  const streamingMessageKey = ref<string | null>(null);

  // Every state bucket is keyed by ownerType + ownerId + topicId.  The same
  // topic/message id can therefore safely exist in Agent and Group scopes.
  const sessionActiveStreams = ref<Record<string, string[]>>({});
  const pendingGenerationRequests = ref<Record<string, string[]>>({});
  const activeStreamMessages = reactive<Map<string, ChatMessage>>(new Map());
  const rAFPendingUpdates = new Map<string, any>();
  const cleanupTimers = new Set<ReturnType<typeof setTimeout>>();

  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const avatarStore = useAvatarStore();
  const topicStore = useTopicStore();

  const runtimeState: StreamState = {
    activeStreamMessages,
    rAFPendingUpdates,
    cleanupTimers,
    streamingMessageId,
    streamingMessageKey,
  };

  const computeShell = createStreamShellFactory(assistantStore, avatarStore);

  const currentIdentity = () => currentConversationIdentity(sessionStore);

  function explicitIdentity(
    ownerId: string,
    ownerType: string,
    topicId: string,
  ): ConversationIdentity | null {
    return makeConversationIdentity(ownerId, ownerType, topicId);
  }

  /** Legacy UI adapters must match the active typed session exactly. */
  function legacySession(
    ownerId: string,
    topicId: string,
  ): ConversationIdentity | null {
    const current = currentIdentity();
    if (
      !current ||
      current.ownerId !== ownerId ||
      current.topicId !== topicId
    ) {
      return null;
    }
    return current;
  }

  function activeMessageKey(
    identity: ConversationIdentity,
    messageId: string,
  ): string | null {
    const message = makeMessageIdentity(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      messageId,
    );
    return message ? messageIdentityKey(message) : null;
  }

  const activeStreamSets = computed(() => {
    const sets: Record<string, Set<string>> = {};
    for (const [key, streams] of Object.entries(sessionActiveStreams.value)) {
      sets[key] = new Set(streams);
    }
    return sets;
  });

  const activeStreamIdSet = computed(() => {
    const ids = new Set<string>();
    for (const streams of Object.values(sessionActiveStreams.value)) {
      streams.forEach((id) => ids.add(id));
    }
    return ids;
  });

  const activeStreamKeySet = computed(() => {
    const keys = new Set<string>();
    for (const [topicKey, streams] of Object.entries(
      sessionActiveStreams.value,
    )) {
      streams.forEach((messageId) =>
        keys.add(`${topicKey}:${encodeURIComponent(messageId)}`),
      );
    }
    return keys;
  });

  function isMessageActiveInIdentity(
    identity: ConversationIdentity,
    messageId: string,
  ): boolean {
    return (
      activeStreamSets.value[topicIdentityKey(identity)]?.has(messageId) ??
      false
    );
  }

  function isMessageActive(
    messageId: string,
    ownerId?: string,
    ownerType?: ConversationOwnerType,
    topicId?: string,
  ): boolean {
    const identity =
      ownerId && ownerType && topicId
        ? explicitIdentity(ownerId, ownerType, topicId)
        : currentIdentity();
    return !!identity && isMessageActiveInIdentity(identity, messageId);
  }

  /**
   * Four arguments are the composite API.  The three-argument form is kept
   * for old renderers but only resolves against the current typed session;
   * missing owner type never defaults to Agent.
   */
  function isMessageActiveInSession(
    ownerId: string,
    ownerTypeOrTopicId: string,
    topicIdOrMessageId: string,
    maybeMessageId?: string,
  ): boolean {
    if (maybeMessageId !== undefined) {
      const identity = explicitIdentity(
        ownerId,
        ownerTypeOrTopicId,
        topicIdOrMessageId,
      );
      return !!identity && isMessageActiveInIdentity(identity, maybeMessageId);
    }
    const identity = legacySession(ownerId, ownerTypeOrTopicId);
    return (
      !!identity && isMessageActiveInIdentity(identity, topicIdOrMessageId)
    );
  }

  const activeStreamingIds = computed(() => {
    const identity = currentIdentity();
    if (!identity) return new Set<string>();
    const key = topicIdentityKey(identity);
    return new Set([
      ...(sessionActiveStreams.value[key] || []),
      ...(pendingGenerationRequests.value[key] || []),
    ]);
  });

  const globalActiveStreamMessageIds = computed(() => activeStreamIdSet.value);

  const isGroupGenerating = computed(() => {
    const identity = currentIdentity();
    if (!identity || identity.ownerType !== "group") return false;
    const key = topicIdentityKey(identity);
    return (
      !!sessionActiveStreams.value[key]?.length ||
      !!pendingGenerationRequests.value[key]?.length
    );
  });

  const hasAnySessionActiveStreams = () =>
    Object.values(sessionActiveStreams.value).some(
      (streams) => streams.length > 0,
    );

  const hasAnyPendingGenerations = () =>
    Object.values(pendingGenerationRequests.value).some(
      (requests) => requests.length > 0,
    );

  const releaseScreenKeepIfIdle = () => {
    if (!hasAnySessionActiveStreams() && !hasAnyPendingGenerations())
      releaseScreenKeep();
  };

  const hasActiveStreams = computed(
    () => hasAnySessionActiveStreams() || hasAnyPendingGenerations(),
  );

  const MAX_STREAM_MESSAGES = 100;
  function enforceStreamPoolLimit(): void {
    if (activeStreamMessages.size <= MAX_STREAM_MESSAGES) return;
    let remaining = activeStreamMessages.size - MAX_STREAM_MESSAGES;
    for (const [messageKey] of activeStreamMessages) {
      if (remaining <= 0) break;
      if (!activeStreamKeySet.value.has(messageKey)) {
        activeStreamMessages.delete(messageKey);
        remaining -= 1;
      }
    }
  }

  function isMessageInAnyActiveStream(
    messageId: string,
    ownerId?: string,
    ownerType?: ConversationOwnerType,
    topicId?: string,
  ): boolean {
    const identity =
      ownerId && ownerType && topicId
        ? explicitIdentity(ownerId, ownerType, topicId)
        : currentIdentity();
    return !!identity && isMessageActiveInIdentity(identity, messageId);
  }

  function addSessionStream(
    identity: ConversationIdentity,
    messageId: string,
  ): void {
    const key = topicIdentityKey(identity);
    const streams =
      sessionActiveStreams.value[key] || (sessionActiveStreams.value[key] = []);
    if (!streams.includes(messageId)) streams.push(messageId);
    enforceStreamPoolLimit();
  }

  function addPendingGeneration(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    requestId: string,
  ): void {
    const identity = explicitIdentity(ownerId, ownerType, topicId);
    if (!identity) return;
    const key = topicIdentityKey(identity);
    const requests =
      pendingGenerationRequests.value[key] ||
      (pendingGenerationRequests.value[key] = []);
    if (!requests.includes(requestId)) requests.push(requestId);
  }

  function removePendingGeneration(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    requestId: string,
  ): void {
    const identity = explicitIdentity(ownerId, ownerType, topicId);
    if (!identity) return;
    const key = topicIdentityKey(identity);
    const requests = pendingGenerationRequests.value[key];
    if (!requests) return;
    const index = requests.indexOf(requestId);
    if (index !== -1) requests.splice(index, 1);
    if (requests.length === 0) delete pendingGenerationRequests.value[key];
    releaseScreenKeepIfIdle();
  }

  function removeSessionStream(
    identity: ConversationIdentity,
    messageId: string,
  ): void {
    const topicKey = topicIdentityKey(identity);
    const streams = sessionActiveStreams.value[topicKey];
    if (streams) {
      const index = streams.indexOf(messageId);
      if (index !== -1) streams.splice(index, 1);
      if (streams.length === 0) delete sessionActiveStreams.value[topicKey];
    }
    releaseScreenKeepIfIdle();

    const messageKey = activeMessageKey(identity, messageId);
    if (!messageKey) return;
    const cleanupTimer = setTimeout(() => {
      cleanupTimers.delete(cleanupTimer);
      if (!activeStreamKeySet.value.has(messageKey)) {
        activeStreamMessages.delete(messageKey);
        clearStreamMessageRendering(runtimeState, messageKey, false);
      }
    }, 1000);
    cleanupTimers.add(cleanupTimer);
  }

  const processStreamEvent = createStreamEventProcessor({
    state: runtimeState,
    computeShell,
    addSessionStream,
    removeSessionStream,
    incrementTopicMsgCount: (identity) =>
      topicStore.incrementTopicMsgCount(identity),
    incrementTopicUnreadCount: (identity) =>
      topicStore.incrementTopicUnreadCount(identity),
    currentIdentity,
    invoke: (command, args) => invoke(command, args),
    isStreamDebugEnabled,
    recordStreamTrace,
    streamDebugLog,
  });

  function getActiveStreamMessage(
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    messageId: string,
  ): ChatMessage | undefined {
    const identity = explicitIdentity(ownerId, ownerType, topicId);
    const key = identity && activeMessageKey(identity, messageId);
    return key ? activeStreamMessages.get(key) : undefined;
  }

  function getCurrentActiveStreamMessage(
    messageId: string,
  ): ChatMessage | undefined {
    const identity = currentIdentity();
    return identity
      ? getActiveStreamMessage(
          identity.ownerId,
          identity.ownerType,
          identity.topicId,
          messageId,
        )
      : undefined;
  }

  const controls = createStreamControls({
    currentIdentity,
    activeStreamingIds,
    activeStreamMessages,
    runtimeState,
    streamingMessageId,
    streamingMessageKey,
    removeSessionStream,
    activeMessageKey,
  });

  onScopeDispose(() => {
    clearStreamRendering(runtimeState);
    pendingGenerationRequests.value = {};
  });

  return {
    streamingMessageId,
    streamingMessageKey,
    sessionActiveStreams,
    pendingGenerationRequests,
    activeStreamMessages,
    activeStreamingIds,
    globalActiveStreamMessageIds,
    activeStreamIdSet,
    isMessageActive,
    isMessageActiveInSession,
    isGroupGenerating,
    hasActiveStreams,
    isMessageInAnyActiveStream,
    computeShell,
    addPendingGeneration,
    removePendingGeneration,
    addSessionStream,
    removeSessionStream,
    processStreamEvent,
    getActiveStreamMessage,
    getCurrentActiveStreamMessage,
    stopMessage: controls.stopMessage,
    stopGroupTurn: controls.stopGroupTurn,
  };
});
