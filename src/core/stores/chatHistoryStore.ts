import { defineStore } from "pinia";
import { ref } from "vue";
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
  const currentChatHistory = ref<ChatMessage[]>([]);
  const loading = ref(false);
  const historyOffset = ref(0);
  const hasMoreHistory = ref(true);
  const isLoadingHistory = ref(false);
  const preloadedHistory = ref<PreloadedHistory | null>(null);
  const editMessageContent = ref("");
  const editingOriginalMessageId = ref<string | null>(null);

  const sessionStore = useChatSessionStore();
  const streamStore = useChatStreamStore();
  const attachmentStore = useAttachmentStore();
  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const topicStore = useTopicStore();
  const switchGuardStore = useConnectionSwitchGuardStore();

  const currentIdentity = () => currentConversationIdentity(sessionStore);
  const isCurrentIdentity = (identity: ConversationIdentity) =>
    sameConversationIdentity(currentIdentity(), identity);

  const loader = createHistoryLoader({
    currentChatHistory,
    loading,
    historyOffset,
    hasMoreHistory,
    isLoadingHistory,
    preloadedHistory,
    sessionStore,
    streamStore,
    attachmentStore,
    currentIdentity,
    isCurrentIdentity,
  });

  const generation = createHistoryGeneration({
    currentChatHistory,
    editingOriginalMessageId,
    sessionStore,
    streamStore,
    attachmentStore,
    assistantStore,
    settingsStore,
    topicStore,
    switchGuardStore,
    currentIdentity,
    isCurrentIdentity,
  });

  const regeneration = createHistoryRegeneration({
    currentChatHistory,
    sessionStore,
    streamStore,
    topicStore,
    currentIdentity,
    isCurrentIdentity,
    summarizeTopic: generation.summarizeTopic,
  });

  const mutations = createHistoryMutations({
    currentChatHistory,
    sessionStore,
    topicStore,
    currentIdentity,
    isCurrentIdentity,
  });

  return {
    currentChatHistory,
    loading,
    historyOffset,
    hasMoreHistory,
    isLoadingHistory,
    editMessageContent,
    editingOriginalMessageId,
    preloadedHistory,
    preloadHistory: loader.preloadHistory,
    loadHistory: loader.loadHistory,
    loadHistoryPaginated: loader.loadHistoryPaginated,
    loadMoreHistory: loader.loadMoreHistory,
    sendMessage: generation.sendMessage,
    triggerGeneration: generation.triggerGeneration,
    summarizeTopic: generation.summarizeTopic,
    regenerateResponse: regeneration,
    deleteMessage: mutations.deleteMessage,
    deleteAttachment: mutations.deleteAttachment,
    updateMessageContent: mutations.updateMessageContent,
    fetchRawContent: mutations.fetchRawContent,
    persistMessageBlocks: mutations.persistMessageBlocks,
    reRenderMessage: mutations.reRenderMessage,
  };
});
