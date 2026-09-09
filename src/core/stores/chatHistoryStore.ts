import { defineStore } from "pinia";
import { ref, type Ref } from "vue";
import { useChatSessionStore } from "./chatSessionStore";
import { useChatStreamStore } from "./chatStreamStore";
import { useAttachmentStore } from "./attachmentStore";
import { useAssistantStore } from "./assistant";
import { useSettingsStore } from "./settings";
import { useConnectionSwitchGuardStore } from "./connectionSwitchGuard";
import { useTopicStore } from "./topicListManager";
import {
  currentConversationIdentity,
  sameConversationIdentity,
  type ConversationIdentity,
} from "./chatStoreIdentity";
import { createHistoryGeneration } from "./chatHistoryGeneration";
import {
  createHistoryLoader,
  type PreloadedHistory,
} from "./chatHistoryLoader";
import { createHistoryMutations } from "./chatHistoryMutations";
import { createHistoryRegeneration } from "./chatHistoryRegeneration";
import type { ChatMessage } from "../types/chat";

export const useChatHistoryStore = defineStore("chatHistory", () => {
  const context = createChatHistoryStoreContext();
  const loader = createHistoryStoreLoader(context);
  const generation = createHistoryStoreGeneration(context);
  const regeneration = createHistoryStoreRegeneration(
    context,
    loader,
    generation,
  );
  const mutations = createHistoryStoreMutations(context);
  return createChatHistoryStoreApi(
    context,
    loader,
    generation,
    regeneration,
    mutations,
  );
});

interface ChatHistoryStoreContext {
  currentChatHistory: Ref<ChatMessage[]>;
  loading: Ref<boolean>;
  historyOffset: Ref<number>;
  hasMoreHistory: Ref<boolean>;
  isLoadingHistory: Ref<boolean>;
  preloadedHistory: Ref<PreloadedHistory | null>;
  editMessageContent: Ref<string>;
  editingOriginalMessageId: Ref<string | null>;
  sessionStore: ReturnType<typeof useChatSessionStore>;
  streamStore: ReturnType<typeof useChatStreamStore>;
  attachmentStore: ReturnType<typeof useAttachmentStore>;
  assistantStore: ReturnType<typeof useAssistantStore>;
  settingsStore: ReturnType<typeof useSettingsStore>;
  topicStore: ReturnType<typeof useTopicStore>;
  switchGuardStore: ReturnType<typeof useConnectionSwitchGuardStore>;
  currentIdentity: () => ConversationIdentity | null;
  isCurrentIdentity: (identity: ConversationIdentity) => boolean;
}

function createChatHistoryStoreContext(): ChatHistoryStoreContext {
  const sessionStore = useChatSessionStore();
  const context: ChatHistoryStoreContext = {
    currentChatHistory: ref<ChatMessage[]>([]),
    loading: ref(false),
    historyOffset: ref(0),
    hasMoreHistory: ref(true),
    isLoadingHistory: ref(false),
    preloadedHistory: ref<PreloadedHistory | null>(null),
    editMessageContent: ref(""),
    editingOriginalMessageId: ref<string | null>(null),
    sessionStore,
    streamStore: useChatStreamStore(),
    attachmentStore: useAttachmentStore(),
    assistantStore: useAssistantStore(),
    settingsStore: useSettingsStore(),
    topicStore: useTopicStore(),
    switchGuardStore: useConnectionSwitchGuardStore(),
    currentIdentity: () => currentConversationIdentity(sessionStore),
    isCurrentIdentity: () => false,
  };
  context.isCurrentIdentity = (identity) =>
    sameConversationIdentity(context.currentIdentity(), identity);
  return context;
}

function createHistoryStoreLoader(context: ChatHistoryStoreContext) {
  return createHistoryLoader({
    currentChatHistory: context.currentChatHistory,
    loading: context.loading,
    historyOffset: context.historyOffset,
    hasMoreHistory: context.hasMoreHistory,
    isLoadingHistory: context.isLoadingHistory,
    preloadedHistory: context.preloadedHistory,
    sessionStore: context.sessionStore,
    streamStore: context.streamStore,
    attachmentStore: context.attachmentStore,
    currentIdentity: context.currentIdentity,
    isCurrentIdentity: context.isCurrentIdentity,
  });
}

function createHistoryStoreGeneration(context: ChatHistoryStoreContext) {
  return createHistoryGeneration({
    currentChatHistory: context.currentChatHistory,
    editingOriginalMessageId: context.editingOriginalMessageId,
    sessionStore: context.sessionStore,
    streamStore: context.streamStore,
    attachmentStore: context.attachmentStore,
    assistantStore: context.assistantStore,
    settingsStore: context.settingsStore,
    topicStore: context.topicStore,
    switchGuardStore: context.switchGuardStore,
    currentIdentity: context.currentIdentity,
    isCurrentIdentity: context.isCurrentIdentity,
  });
}

function createHistoryStoreRegeneration(
  context: ChatHistoryStoreContext,
  loader: ReturnType<typeof createHistoryLoader>,
  generation: ReturnType<typeof createHistoryGeneration>,
) {
  return createHistoryRegeneration({
    currentChatHistory: context.currentChatHistory,
    sessionStore: context.sessionStore,
    streamStore: context.streamStore,
    topicStore: context.topicStore,
    currentIdentity: context.currentIdentity,
    isCurrentIdentity: context.isCurrentIdentity,
    summarizeTopic: generation.summarizeTopic,
    reloadCurrentHistory: (identity) =>
      loader.loadHistory(
        identity.ownerId,
        identity.ownerType,
        identity.topicId,
        15,
        0,
      ),
  });
}

function createHistoryStoreMutations(context: ChatHistoryStoreContext) {
  return createHistoryMutations({
    currentChatHistory: context.currentChatHistory,
    sessionStore: context.sessionStore,
    topicStore: context.topicStore,
    currentIdentity: context.currentIdentity,
    isCurrentIdentity: context.isCurrentIdentity,
    cancelUnreadReceipt: context.streamStore.cancelUnreadReceipt,
  });
}

function createChatHistoryStoreApi(
  context: ChatHistoryStoreContext,
  loader: ReturnType<typeof createHistoryLoader>,
  generation: ReturnType<typeof createHistoryGeneration>,
  regeneration: ReturnType<typeof createHistoryRegeneration>,
  mutations: ReturnType<typeof createHistoryMutations>,
) {
  return {
    currentChatHistory: context.currentChatHistory,
    loading: context.loading,
    historyOffset: context.historyOffset,
    hasMoreHistory: context.hasMoreHistory,
    isLoadingHistory: context.isLoadingHistory,
    editMessageContent: context.editMessageContent,
    editingOriginalMessageId: context.editingOriginalMessageId,
    preloadedHistory: context.preloadedHistory,
    preloadHistory: loader.preloadHistory,
    loadHistory: loader.loadHistory,
    loadHistoryPaginated: loader.loadHistoryPaginated,
    loadMoreHistory: loader.loadMoreHistory,
    installAnchoredHistory: loader.installAnchoredHistory,
    sendMessage: generation.sendMessage,
    triggerGeneration: generation.triggerGeneration,
    summarizeTopic: generation.summarizeTopic,
    regenerateResponse: regeneration,
    ...mutations,
  };
}
