import type { Ref } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import type { useChatSessionStore } from "./chatSessionStore";
import type { useNotificationStore } from "./notification";
import type { Topic } from "./topicTypes";

export interface TopicStoreContext {
  topics: Ref<Topic[]>;
  loading: Ref<boolean>;
  currentAgentId: Ref<string | null>;
  currentOwnerKey: Ref<string | null>;
  sessionStore: ReturnType<typeof useChatSessionStore>;
  notificationStore: ReturnType<typeof useNotificationStore>;
}

function hasTopicIdentity(
  topic: Topic,
  ownerId: string,
  ownerType: "agent" | "group",
  topicId: string,
) {
  return (
    topic.id === topicId &&
    topic.ownerId === ownerId &&
    topic.ownerType === ownerType
  );
}

function isCurrentSessionTopic(
  context: TopicStoreContext,
  ownerId: string,
  ownerType: "agent" | "group",
  topicId: string,
) {
  return (
    context.sessionStore.currentTopicId === topicId &&
    context.sessionStore.currentSelectedItem?.id === ownerId &&
    context.sessionStore.currentSelectedItem?.type === ownerType
  );
}

const mapTopicChunk = (
  chunk: Topic[],
  ownerId: string,
  ownerType: "agent" | "group",
) =>
  chunk.map((topic) => ({
    ...topic,
    ownerId,
    ownerType,
    name: topic.name || (topic as any).title || topic.id,
    unreadCount: (topic as any).unreadCount || 0,
    msgCount: (topic as any).msgCount || 0,
  }));

const loadTopicList = async (
  context: TopicStoreContext,
  ownerId: string,
  owner_type: string,
) => {
  if (!ownerId) return;
  if (owner_type !== "agent" && owner_type !== "group") {
    throw new Error(`Unsupported topic owner type: ${owner_type}`);
  }
  const ownerType: "agent" | "group" = owner_type;
  context.currentAgentId.value = ownerId;
  const requestOwnerKey = `${ownerType}:${ownerId}`;
  context.currentOwnerKey.value = requestOwnerKey;
  console.log(`[TopicStore] Loading topics for ${ownerType}: ${ownerId}`);
  context.loading.value = true;

  try {
    const channel = new Channel<Topic[]>();
    context.topics.value = [];
    channel.onmessage = (chunk) => {
      if (context.currentOwnerKey.value !== requestOwnerKey) return;
      context.topics.value.push(...mapTopicChunk(chunk, ownerId, ownerType));
      context.topics.value = [...context.topics.value];
    };
    await invoke("get_topics_streamed", {
      ownerId,
      ownerType,
      onChunk: channel,
    });
    console.log(`[TopicStore] Topic list streaming completed for ${ownerId}`);
  } catch (error) {
    console.error("[TopicStore] Failed to load topics:", error);
  } finally {
    if (context.currentOwnerKey.value === requestOwnerKey) {
      context.loading.value = false;
    }
  }
};

const createTopic = async (
  context: TopicStoreContext,
  ownerId: string,
  ownerType: "agent" | "group",
  name: string,
) => {
  try {
    console.log(
      `[TopicStore] Creating new topic "${name}" for ${ownerType} ${ownerId}`,
    );
    const newTopic = await invoke<Topic>("create_topic", {
      ownerId,
      ownerType,
      name,
    });
    const topicWithState: Topic = {
      ...newTopic,
      ownerId,
      ownerType,
      unreadCount: 0,
      msgCount: 0,
      unread: false,
      locked: true,
    };
    if (context.currentOwnerKey.value === `${ownerType}:${ownerId}`) {
      context.topics.value.unshift(topicWithState);
      context.topics.value = [...context.topics.value];
    }
    context.notificationStore.addNotification({
      type: "success",
      title: "话题创建成功",
      message: `已开启新话题: ${name}`,
      toastOnly: true,
    });
    return topicWithState;
  } catch (error: any) {
    console.error("[TopicStore] Failed to create topic:", error);
    context.notificationStore.addNotification({
      type: "error",
      title: "创建话题失败",
      message:
        typeof error === "string"
          ? error
          : error.message || "系统或网络异常，请稍后重试",
      duration: 5000,
    });
    throw error;
  }
};

const deleteTopic = async (
  context: TopicStoreContext,
  ownerId: string,
  ownerType: "agent" | "group",
  topicId: string,
) => {
  try {
    console.log(`[TopicStore] Deleting topic ${topicId}`);
    await invoke("delete_topic", { ownerId, ownerType, topicId });
    context.topics.value = context.topics.value.filter(
      (topic) => !hasTopicIdentity(topic, ownerId, ownerType, topicId),
    );
    context.notificationStore.addNotification({
      type: "success",
      title: "话题删除成功",
      message: "话题及其记录已被移除",
      toastOnly: true,
    });
    if (!isCurrentSessionTopic(context, ownerId, ownerType, topicId)) return;
    const nextTopic = context.topics.value[0];
    if (nextTopic) {
      await context.sessionStore.selectTopicById(
        nextTopic.ownerId,
        nextTopic.ownerType,
        nextTopic.id,
      );
    } else {
      context.sessionStore.currentTopicId = null;
    }
  } catch (error) {
    console.error("[TopicStore] Failed to delete topic:", error);
    throw error;
  }
};

const updateTopicTitle = async (
  context: TopicStoreContext,
  ownerId: string,
  ownerType: "agent" | "group",
  topicId: string,
  newTitle: string,
) => {
  try {
    console.log(
      `[TopicStore] Updating title for topic ${topicId} to "${newTitle}"`,
    );
    await invoke("update_topic_title", {
      ownerId,
      ownerType,
      topicId,
      title: newTitle,
    });
    const index = context.topics.value.findIndex(
      (topic) => hasTopicIdentity(topic, ownerId, ownerType, topicId),
    );
    if (index !== -1) {
      context.topics.value[index] = {
        ...context.topics.value[index],
        name: newTitle,
      };
      context.topics.value = [...context.topics.value];
    }
    if (
      isCurrentSessionTopic(context, ownerId, ownerType, topicId) &&
      context.sessionStore.currentSelectedItem
    ) {
      context.sessionStore.currentSelectedItem.name = newTitle;
    }
  } catch (error) {
    console.error("[TopicStore] Failed to update topic title:", error);
    throw error;
  }
};

const toggleTopicLock = async (
  context: TopicStoreContext,
  ownerId: string,
  ownerType: "agent" | "group",
  topicId: string,
) => {
  try {
    const index = context.topics.value.findIndex(
      (topic) => hasTopicIdentity(topic, ownerId, ownerType, topicId),
    );
    if (index === -1) return;
    const targetLockState = !context.topics.value[index].locked;
    await invoke("toggle_topic_lock", {
      ownerId,
      ownerType,
      topicId,
      locked: targetLockState,
    });
    context.topics.value[index] = {
      ...context.topics.value[index],
      locked: targetLockState,
    };
    context.topics.value = [...context.topics.value];
  } catch (error) {
    console.error("[TopicStore] Failed to toggle topic lock:", error);
    throw error;
  }
};

const setTopicUnread = async (
  context: TopicStoreContext,
  ownerId: string,
  ownerType: "agent" | "group",
  topicId: string,
  unread: boolean,
) => {
  try {
    await invoke("set_topic_unread", { ownerId, ownerType, topicId, unread });
    const index = context.topics.value.findIndex(
      (topic) => hasTopicIdentity(topic, ownerId, ownerType, topicId),
    );
    if (index === -1) return;
    context.topics.value[index] = {
      ...context.topics.value[index],
      unread,
    };
    context.topics.value = [...context.topics.value];
  } catch (error) {
    console.error("[TopicStore] Failed to set topic unread:", error);
    throw error;
  }
};

const invalidateAllTopicCaches = (context: TopicStoreContext) => {
  context.topics.value = [];
  console.log("[TopicStore] All topic caches invalidated");
};

export function createTopicListActions(context: TopicStoreContext) {
  return {
    loadTopicList: (ownerId: string, ownerType: string) =>
      loadTopicList(context, ownerId, ownerType),
    createTopic: (
      ownerId: string,
      ownerType: "agent" | "group",
      name: string,
    ) => createTopic(context, ownerId, ownerType, name),
    deleteTopic: (
      ownerId: string,
      ownerType: "agent" | "group",
      topicId: string,
    ) => deleteTopic(context, ownerId, ownerType, topicId),
    updateTopicTitle: (
      ownerId: string,
      ownerType: "agent" | "group",
      topicId: string,
      newTitle: string,
    ) => updateTopicTitle(context, ownerId, ownerType, topicId, newTitle),
    toggleTopicLock: (
      ownerId: string,
      ownerType: "agent" | "group",
      topicId: string,
    ) => toggleTopicLock(context, ownerId, ownerType, topicId),
    setTopicUnread: (
      ownerId: string,
      ownerType: "agent" | "group",
      topicId: string,
      unread: boolean,
    ) => setTopicUnread(context, ownerId, ownerType, topicId, unread),
    invalidateAllTopicCaches: () => invalidateAllTopicCaches(context),
  };
}
