import { defineStore } from "pinia";
import { ref } from "vue";
import { useChatSessionStore } from "./chatSessionStore";
import { useNotificationStore } from "./notification";
import { useTopicListCounters } from "./topicListCounters";
import {
  createTopicListActions,
  type TopicStoreContext,
} from "./topicListManagerActions";
import { createFilteredTopics } from "./topicListManagerGetters";
import type { Topic } from "./topicTypes";
export type { Topic } from "./topicTypes";

export const useTopicStore = defineStore("topic", () => {
  const sessionStore = useChatSessionStore();
  const notificationStore = useNotificationStore();
  const topics = ref<Topic[]>([]);
  const loading = ref(false);
  const searchTerm = ref("");
  const currentAgentId = ref<string | null>(null);
  const currentOwnerKey = ref<string | null>(null);
  const context: TopicStoreContext = {
    topics,
    loading,
    currentAgentId,
    currentOwnerKey,
    sessionStore,
    notificationStore,
  };
  const filteredTopics = createFilteredTopics(topics, searchTerm);
  const actions = createTopicListActions(context);
  const topicCounters = useTopicListCounters(topics, sessionStore);

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
