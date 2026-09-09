import { computed, type ComputedRef } from "vue";
import { releaseScreenKeep } from "../composables/useScreenKeeper";
import { clearStreamMessageRendering } from "./chatStreamProcessor";
import {
  topicIdentityKey,
  type ConversationIdentity,
  type ConversationOwnerType,
} from "./chatStoreIdentity";
import type { ChatStreamStoreState } from "./chatStreamStoreState";
import type { ChatStreamIdentityApi } from "./chatStreamStoreIdentity";

export interface ChatStreamActivityApi {
  activeStreamingIds: ComputedRef<Set<string>>;
  globalActiveStreamMessageIds: ComputedRef<Set<string>>;
  activeStreamIdSet: ComputedRef<Set<string>>;
  isMessageActive: (
    messageId: string,
    ownerId?: string,
    ownerType?: ConversationOwnerType,
    topicId?: string,
  ) => boolean;
  isMessageActiveInSession: (
    ownerId: string,
    ownerTypeOrTopicId: string,
    topicIdOrMessageId: string,
    maybeMessageId?: string,
  ) => boolean;
  isGroupGenerating: ComputedRef<boolean>;
  hasActiveStreams: ComputedRef<boolean>;
  isMessageInAnyActiveStream: (
    messageId: string,
    ownerId?: string,
    ownerType?: ConversationOwnerType,
    topicId?: string,
  ) => boolean;
  addPendingGeneration: (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    requestId: string,
  ) => void;
  removePendingGeneration: (
    ownerId: string,
    ownerType: ConversationOwnerType,
    topicId: string,
    requestId: string,
  ) => void;
  addSessionStream: (identity: ConversationIdentity, messageId: string) => void;
  removeSessionStream: (
    identity: ConversationIdentity,
    messageId: string,
  ) => void;
}

interface ActivityDeps {
  state: ChatStreamStoreState;
  identity: ChatStreamIdentityApi;
}

const MAX_STREAM_MESSAGES = 100;

function createActiveStreamSets(state: ChatStreamStoreState) {
  return computed(() => {
    const sets: Record<string, Set<string>> = {};
    for (const [key, streams] of Object.entries(
      state.sessionActiveStreams.value,
    )) {
      sets[key] = new Set(streams);
    }
    return sets;
  });
}

function createActiveStreamIdSet(state: ChatStreamStoreState) {
  return computed(() => {
    const ids = new Set<string>();
    for (const streams of Object.values(state.sessionActiveStreams.value)) {
      streams.forEach((id) => ids.add(id));
    }
    return ids;
  });
}

function isMessageActiveInIdentity(
  activeStreamSets: ReturnType<typeof createActiveStreamSets>,
  identity: ConversationIdentity,
  messageId: string,
): boolean {
  return (
    activeStreamSets.value[topicIdentityKey(identity)]?.has(messageId) ?? false
  );
}

function hasAnyStreams(state: ChatStreamStoreState): boolean {
  return Object.values(state.sessionActiveStreams.value).some(
    (streams) => streams.length > 0,
  );
}

function hasAnyPending(state: ChatStreamStoreState): boolean {
  return Object.values(state.pendingGenerationRequests.value).some(
    (requests) => requests.length > 0,
  );
}

function enforceStreamPoolLimit(
  state: ChatStreamStoreState,
  activeStreamKeySet: ComputedRef<Set<string>>,
): void {
  if (state.activeStreamMessages.size <= MAX_STREAM_MESSAGES) return;
  let remaining = state.activeStreamMessages.size - MAX_STREAM_MESSAGES;
  for (const [messageKey] of state.activeStreamMessages) {
    if (remaining <= 0) break;
    if (!activeStreamKeySet.value.has(messageKey)) {
      state.activeStreamMessages.delete(messageKey);
      remaining -= 1;
    }
  }
}

function addSessionStream(
  deps: ActivityDeps,
  identity: ConversationIdentity,
  messageId: string,
  activeStreamKeySet: ComputedRef<Set<string>>,
): void {
  const key = topicIdentityKey(identity);
  const streams =
    deps.state.sessionActiveStreams.value[key] ||
    (deps.state.sessionActiveStreams.value[key] = []);
  if (!streams.includes(messageId)) streams.push(messageId);
  enforceStreamPoolLimit(deps.state, activeStreamKeySet);
}

function removeSessionStream(
  deps: ActivityDeps,
  identity: ConversationIdentity,
  messageId: string,
  activeStreamKeySet: ComputedRef<Set<string>>,
): void {
  const topicKey = topicIdentityKey(identity);
  const streams = deps.state.sessionActiveStreams.value[topicKey];
  let removed = false;
  if (streams) {
    const index = streams.indexOf(messageId);
    if (index !== -1) {
      streams.splice(index, 1);
      removed = true;
    }
    if (streams.length === 0)
      delete deps.state.sessionActiveStreams.value[topicKey];
  }
  if (removed && !hasAnyStreams(deps.state) && !hasAnyPending(deps.state)) {
    releaseScreenKeep();
  }
  scheduleMessageCleanup(deps, identity, messageId, activeStreamKeySet);
}

function scheduleMessageCleanup(
  deps: ActivityDeps,
  identity: ConversationIdentity,
  messageId: string,
  activeStreamKeySet: ComputedRef<Set<string>>,
): void {
  const messageKey = deps.identity.activeMessageKey(identity, messageId);
  if (!messageKey) return;
  const cleanupTimer = setTimeout(() => {
    deps.state.cleanupTimers.delete(cleanupTimer);
    if (!activeStreamKeySet.value.has(messageKey)) {
      deps.state.activeStreamMessages.delete(messageKey);
      clearStreamMessageRendering(deps.state.runtimeState, messageKey, false);
    }
  }, 1000);
  deps.state.cleanupTimers.add(cleanupTimer);
}

function addPendingGeneration(
  deps: ActivityDeps,
  ownerId: string,
  ownerType: ConversationOwnerType,
  topicId: string,
  requestId: string,
): void {
  const identity = deps.identity.explicitIdentity(ownerId, ownerType, topicId);
  if (!identity) return;
  const key = topicIdentityKey(identity);
  const requests =
    deps.state.pendingGenerationRequests.value[key] ||
    (deps.state.pendingGenerationRequests.value[key] = []);
  if (!requests.includes(requestId)) requests.push(requestId);
}

function removePendingGeneration(
  deps: ActivityDeps,
  ownerId: string,
  ownerType: ConversationOwnerType,
  topicId: string,
  requestId: string,
): void {
  const identity = deps.identity.explicitIdentity(ownerId, ownerType, topicId);
  if (!identity) return;
  const key = topicIdentityKey(identity);
  const requests = deps.state.pendingGenerationRequests.value[key];
  if (!requests) return;
  const index = requests.indexOf(requestId);
  if (index !== -1) requests.splice(index, 1);
  if (requests.length === 0)
    delete deps.state.pendingGenerationRequests.value[key];
  if (!hasAnyStreams(deps.state) && !hasAnyPending(deps.state)) {
    releaseScreenKeep();
  }
}

function resolveMessageIdentity(
  identity: ChatStreamIdentityApi,
  ownerId?: string,
  ownerType?: ConversationOwnerType,
  topicId?: string,
): ConversationIdentity | null {
  return ownerId && ownerType && topicId
    ? identity.explicitIdentity(ownerId, ownerType, topicId)
    : identity.currentIdentity();
}

function createMessageActivityApi(
  deps: ActivityDeps,
  activeStreamSets: ReturnType<typeof createActiveStreamSets>,
) {
  const isMessageActive = (
    messageId: string,
    ownerId?: string,
    ownerType?: ConversationOwnerType,
    topicId?: string,
  ) => {
    const resolved = resolveMessageIdentity(
      deps.identity,
      ownerId,
      ownerType,
      topicId,
    );
    return (
      !!resolved &&
      isMessageActiveInIdentity(activeStreamSets, resolved, messageId)
    );
  };

  const isMessageActiveInSession = (
    ownerId: string,
    ownerTypeOrTopicId: string,
    topicIdOrMessageId: string,
    maybeMessageId?: string,
  ) => {
    const resolved =
      maybeMessageId !== undefined
        ? deps.identity.explicitIdentity(
            ownerId,
            ownerTypeOrTopicId,
            topicIdOrMessageId,
          )
        : deps.identity.legacySession(ownerId, ownerTypeOrTopicId);
    const messageId = maybeMessageId ?? topicIdOrMessageId;
    return (
      !!resolved &&
      isMessageActiveInIdentity(activeStreamSets, resolved, messageId)
    );
  };

  return { isMessageActive, isMessageActiveInSession };
}

export function createChatStreamActivity(
  deps: ActivityDeps,
): ChatStreamActivityApi {
  const activeStreamSets = createActiveStreamSets(deps.state);
  const activeStreamIdSet = createActiveStreamIdSet(deps.state);
  const activeStreamKeySet = computed(() => {
    const keys = new Set<string>();
    for (const [topicKey, streams] of Object.entries(
      deps.state.sessionActiveStreams.value,
    )) {
      streams.forEach((messageId) =>
        keys.add(`${topicKey}:${encodeURIComponent(messageId)}`),
      );
    }
    return keys;
  });
  const messageActivity = createMessageActivityApi(deps, activeStreamSets);
  const activeStreamingIds = computed(() => {
    const identity = deps.identity.currentIdentity();
    if (!identity) return new Set<string>();
    const key = topicIdentityKey(identity);
    return new Set([
      ...(deps.state.sessionActiveStreams.value[key] || []),
      ...(deps.state.pendingGenerationRequests.value[key] || []),
    ]);
  });
  const hasActiveStreams = computed(
    () => hasAnyStreams(deps.state) || hasAnyPending(deps.state),
  );
  const isGroupGenerating = computed(() => {
    const identity = deps.identity.currentIdentity();
    if (!identity || identity.ownerType !== "group") return false;
    const key = topicIdentityKey(identity);
    return (
      !!deps.state.sessionActiveStreams.value[key]?.length ||
      !!deps.state.pendingGenerationRequests.value[key]?.length
    );
  });
  return {
    ...messageActivity,
    activeStreamingIds,
    globalActiveStreamMessageIds: activeStreamIdSet,
    activeStreamIdSet,
    isGroupGenerating,
    hasActiveStreams,
    isMessageInAnyActiveStream: messageActivity.isMessageActive,
    addPendingGeneration: (ownerId, ownerType, topicId, requestId) =>
      addPendingGeneration(deps, ownerId, ownerType, topicId, requestId),
    removePendingGeneration: (ownerId, ownerType, topicId, requestId) =>
      removePendingGeneration(deps, ownerId, ownerType, topicId, requestId),
    addSessionStream: (identity, messageId) =>
      addSessionStream(deps, identity, messageId, activeStreamKeySet),
    removeSessionStream: (identity, messageId) =>
      removeSessionStream(deps, identity, messageId, activeStreamKeySet),
  };
}
