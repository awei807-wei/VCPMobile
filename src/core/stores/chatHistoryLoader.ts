import { invoke } from "@tauri-apps/api/core";
import type { Ref } from "vue";
import type { ChatMessage } from "../types/chat";
import {
  makeConversationIdentity,
  topicIdentityKey,
  type ConversationIdentity,
  type ConversationOwnerType,
} from "./chatStoreIdentity";
import {
  isActiveHistoryRequest,
  shouldIgnoreHistoryPage,
  type HistoryLoaderState,
} from "./chatHistoryLoaderSupport";
import { loadStreamedHistory } from "./historyStreamLoader";

export interface PreloadedHistory {
  ownerId: string;
  ownerType: ConversationOwnerType;
  topicId: string;
  messages: ChatMessage[];
}

export interface AnchoredHistoryWindow {
  messages: ChatMessage[];
  nextOffset: number;
  hasMoreHistory: boolean;
}

export interface HistoryLoaderDeps {
  currentChatHistory: Ref<ChatMessage[]>;
  loading: Ref<boolean>;
  historyOffset: Ref<number>;
  hasMoreHistory: Ref<boolean>;
  isLoadingHistory: Ref<boolean>;
  preloadedHistory: Ref<PreloadedHistory | null>;
  sessionStore: {
    currentTopicId: string | null;
    currentSelectedItem: { id?: string; type?: string } | null;
  };
  streamStore: {
    getActiveStreamMessage: (
      ownerId: string,
      ownerType: ConversationOwnerType,
      topicId: string,
      messageId: string,
    ) => ChatMessage | undefined;
  };
  attachmentStore: {
    resolveMessageAssets: (message: ChatMessage) => void;
  };
  currentIdentity: () => ConversationIdentity | null;
  isCurrentIdentity: (identity: ConversationIdentity) => boolean;
}

function beginHistoryEpoch(
  state: HistoryLoaderState,
  identity: ConversationIdentity,
): number {
  state.historyEpoch += 1;
  state.activeHistoryEpoch = state.historyEpoch;
  state.activeHistoryIdentity = identity;
  return state.activeHistoryEpoch;
}

async function preloadHistory(
  deps: HistoryLoaderDeps,
  state: HistoryLoaderState,
  ownerId: string,
  ownerType: string,
  topicId: string,
  limit = 5,
): Promise<void> {
  const identity = makeConversationIdentity(ownerId, ownerType, topicId);
  if (!identity) {
    deps.preloadedHistory.value = null;
    return;
  }
  const requestEpoch = ++state.preloadRequestEpoch;
  try {
    const messages = await invoke<ChatMessage[]>("load_chat_history", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      limit,
      offset: 0,
    });
    if (requestEpoch !== state.preloadRequestEpoch) return;
    deps.preloadedHistory.value = {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      messages,
    };
    console.log(
      `[ChatHistoryStore] Preloaded ${messages.length} messages for ${topicIdentityKey(identity)}`,
    );
  } catch (error) {
    console.error("[ChatHistoryStore] Preload failed:", error);
    if (requestEpoch === state.preloadRequestEpoch)
      deps.preloadedHistory.value = null;
  }
}

async function loadInitialMessages(
  deps: HistoryLoaderDeps,
  identity: ConversationIdentity,
  limit: number,
  offset: number,
): Promise<ChatMessage[]> {
  if (
    deps.preloadedHistory.value &&
    topicIdentityKey(deps.preloadedHistory.value) === topicIdentityKey(identity)
  ) {
    const messages = deps.preloadedHistory.value.messages;
    deps.preloadedHistory.value = null;
    return messages;
  }
  return invoke<ChatMessage[]>("load_chat_history", {
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
    limit,
    offset,
  });
}

function hydrateHistoryMessages(
  deps: HistoryLoaderDeps,
  identity: ConversationIdentity,
  messages: ChatMessage[],
): ChatMessage[] {
  return messages.map(
    (message) =>
      deps.streamStore.getActiveStreamMessage(
        identity.ownerId,
        identity.ownerType,
        identity.topicId,
        message.id,
      ) || message,
  );
}

async function loadInitialHistory(
  deps: HistoryLoaderDeps,
  state: HistoryLoaderState,
  identity: ConversationIdentity,
  limit: number,
  offset: number,
  requestEpoch: number,
  signal: AbortSignal,
): Promise<void> {
  const messages = await loadInitialMessages(deps, identity, limit, offset);
  if (!isActiveHistoryRequest(deps, state, identity, requestEpoch, signal))
    return;

  const hydrated = hydrateHistoryMessages(deps, identity, messages);
  deps.currentChatHistory.value = hydrated;
  deps.historyOffset.value = hydrated.length;
  deps.hasMoreHistory.value = hydrated.length >= limit;
  hydrated.forEach((message) =>
    deps.attachmentStore.resolveMessageAssets(message),
  );
}

function beginHistoryLoad(state: HistoryLoaderState): AbortController {
  state.currentLoadAbortController?.abort();
  const controller = new AbortController();
  state.currentLoadAbortController = controller;
  return controller;
}

async function loadHistory(
  deps: HistoryLoaderDeps,
  state: HistoryLoaderState,
  ownerId: string,
  ownerType: string,
  topicId: string,
  limit = 15,
  offset = 0,
  epoch?: number,
): Promise<void> {
  const identity = makeConversationIdentity(ownerId, ownerType, topicId);
  if (!identity) return;
  const requestEpoch =
    offset === 0
      ? (epoch ?? beginHistoryEpoch(state, identity))
      : (epoch ?? state.activeHistoryEpoch);
  if (shouldIgnoreHistoryPage(state, identity, offset, requestEpoch)) return;

  deps.loading.value = true;
  deps.isLoadingHistory.value = true;
  const controller = beginHistoryLoad(state);
  const { signal } = controller;
  try {
    if (offset === 0) {
      await loadInitialHistory(
        deps,
        state,
        identity,
        limit,
        offset,
        requestEpoch,
        signal,
      );
    } else {
      await loadStreamedHistory({
        deps,
        state,
        identity,
        limit,
        offset,
        requestEpoch,
        signal,
      });
    }
  } catch (error) {
    console.error("[ChatHistoryStore] Failed to stream history:", error);
  } finally {
    if (
      state.currentLoadAbortController === controller &&
      requestEpoch === state.activeHistoryEpoch
    ) {
      state.currentLoadAbortController = null;
      deps.loading.value = false;
      deps.isLoadingHistory.value = false;
    }
  }
}

async function loadHistoryPaginated(
  deps: HistoryLoaderDeps,
  state: HistoryLoaderState,
  ownerId: string,
  ownerType: string,
  topicId: string,
): Promise<void> {
  const identity = makeConversationIdentity(ownerId, ownerType, topicId);
  if (!identity) return;
  const epoch = beginHistoryEpoch(state, identity);
  deps.historyOffset.value = 0;
  deps.hasMoreHistory.value = true;
  await loadHistory(deps, state, ownerId, ownerType, topicId, 5, 0, epoch);
}

async function loadMoreHistory(
  deps: HistoryLoaderDeps,
  state: HistoryLoaderState,
): Promise<void> {
  if (!deps.hasMoreHistory.value || deps.isLoadingHistory.value) return;
  const identity = deps.currentIdentity();
  if (!identity || state.activeHistoryEpoch === 0) return;
  await loadHistory(
    deps,
    state,
    identity.ownerId,
    identity.ownerType,
    identity.topicId,
    10,
    deps.historyOffset.value,
    state.activeHistoryEpoch,
  );
}

function installAnchoredHistory(
  deps: HistoryLoaderDeps,
  state: HistoryLoaderState,
  identity: ConversationIdentity,
  window: AnchoredHistoryWindow,
): boolean {
  if (!deps.isCurrentIdentity(identity)) return false;
  beginHistoryEpoch(state, identity);
  state.currentLoadAbortController?.abort();
  state.currentLoadAbortController = null;
  deps.loading.value = false;
  deps.isLoadingHistory.value = false;
  const hydrated = hydrateHistoryMessages(deps, identity, window.messages);
  deps.currentChatHistory.value = hydrated;
  deps.historyOffset.value = Math.max(window.nextOffset, hydrated.length);
  deps.hasMoreHistory.value = window.hasMoreHistory;
  hydrated.forEach((message) =>
    deps.attachmentStore.resolveMessageAssets(message),
  );
  return true;
}

export function createHistoryLoader(deps: HistoryLoaderDeps) {
  const state: HistoryLoaderState = {
    currentLoadAbortController: null,
    preloadRequestEpoch: 0,
    historyEpoch: 0,
    activeHistoryEpoch: 0,
    activeHistoryIdentity: null,
  };
  return {
    preloadHistory: (
      ownerId: string,
      ownerType: string,
      topicId: string,
      limit = 5,
    ) => preloadHistory(deps, state, ownerId, ownerType, topicId, limit),
    loadHistory: (
      ownerId: string,
      ownerType: string,
      topicId: string,
      limit = 15,
      offset = 0,
      epoch?: number,
    ) =>
      loadHistory(
        deps,
        state,
        ownerId,
        ownerType,
        topicId,
        limit,
        offset,
        epoch,
      ),
    loadHistoryPaginated: (
      ownerId: string,
      ownerType: string,
      topicId: string,
    ) => loadHistoryPaginated(deps, state, ownerId, ownerType, topicId),
    loadMoreHistory: () => loadMoreHistory(deps, state),
    installAnchoredHistory: (
      identity: ConversationIdentity,
      window: AnchoredHistoryWindow,
    ) => installAnchoredHistory(deps, state, identity, window),
  };
}
