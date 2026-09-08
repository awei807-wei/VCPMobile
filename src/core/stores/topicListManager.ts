import { defineStore } from "pinia";
import { ref } from "vue";
import { useChatSessionStore } from "./chatSessionStore";
import { useAssistantStore } from "./assistant";
import { useNotificationStore } from "./notification";
import {
  createTopicUnreadMutationQueue,
  useTopicListCounters,
} from "./topicListCounters";
import {
  createTopicListActions,
  type TopicStoreContext,
} from "./topicListManagerActions";
import { createFilteredTopics } from "./topicListManagerGetters";
import type { Topic } from "./topicTypes";
export type { Topic } from "./topicTypes";

export const useTopicStore = defineStore("topic", () => {
  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const notificationStore = useNotificationStore();
  const topics = ref<Topic[]>([]);
  const loading = ref(false);
  const searchTerm = ref("");
  const currentAgentId = ref<string | null>(null);
  const currentOwnerKey = ref<string | null>(null);
  const requestEpoch = ref(0);
  const unreadMutationQueue = createTopicUnreadMutationQueue();
  const context: TopicStoreContext = {
    topics,
    loading,
    currentAgentId,
    currentOwnerKey,
    requestEpoch,
    sessionStore,
    notificationStore,
    unreadMutationQueue,
  };
  const filteredTopics = createFilteredTopics(topics, searchTerm);
  const actions = createTopicListActions(context);
  const reloadOwnerTopics = async (
    ownerId: string,
    ownerType: "agent" | "group",
  ) => {
    if (context.currentOwnerKey.value !== `${ownerType}:${ownerId}`) return;
    await actions.loadTopicList(ownerId, ownerType);
  };
  context.reloadOwnerTopics = reloadOwnerTopics;
  context.applyOwnerUnreadCount = (ownerId, ownerType, unreadCount) =>
    assistantStore.applyOwnerUnreadCount(ownerId, ownerType, unreadCount);
  context.refreshOwnerUnreadCount = (ownerId, ownerType) =>
    assistantStore.refreshOwnerUnreadCount(ownerId, ownerType);
  const topicCounters = useTopicListCounters(topics, sessionStore, {
    queue: unreadMutationQueue,
    notificationStore,
    reloadOwnerTopics,
    applyOwnerUnreadCount: context.applyOwnerUnreadCount,
    refreshOwnerUnreadCount: context.refreshOwnerUnreadCount,
  });

  return {
    topics,
    loading,
    searchTerm,
    filteredTopics,
    ...actions,
    currentAgentId,
    currentOwnerKey,
    ...topicCounters,
  };
});
