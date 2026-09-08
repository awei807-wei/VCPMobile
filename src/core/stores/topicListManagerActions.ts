import type { Ref } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import type { useChatSessionStore } from "./chatSessionStore";
import type { useNotificationStore } from "./notification";
import {
  applyTopicUnreadState,
  createTopicUnreadMutationQueue,
  reportTopicUnreadMutationFailure,
  validateTopicUnreadState,
  type TopicUnreadMutationQueue,
  type TopicUnreadState,
} from "./topicListCounters";
import type { Topic } from "./topicTypes";
import type { ConversationIdentity } from "./chatStoreIdentity";

export interface TopicStoreContext {
  topics: Ref<Topic[]>;
  loading: Ref<boolean>;
  currentAgentId: Ref<string | null>;
  currentOwnerKey: Ref<string | null>;
  requestEpoch: Ref<number>;
  sessionStore: ReturnType<typeof useChatSessionStore>;
  notificationStore: ReturnType<typeof useNotificationStore>;
  unreadMutationQueue?: TopicUnreadMutationQueue;
  reloadOwnerTopics?: (
    ownerId: string,
    ownerType: "agent" | "group",
  ) => Promise<void>;
  applyOwnerUnreadCount?: (
    ownerId: string,
    ownerType: "agent" | "group",
    unreadCount: number,
  ) => void;
  refreshOwnerUnreadCount?: (
    ownerId: string,
    ownerType: "agent" | "group",
  ) => Promise<void>;
  cancelUnreadReceiptsForTopic?: (identity: ConversationIdentity) => void;
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

function beginTopicRequest(
  context: TopicStoreContext,
  ownerId: string,
  ownerType: "agent" | "group",
): { ownerKey: string; epoch: number } {
  const ownerKey = `${ownerType}:${ownerId}`;
  const epoch = context.requestEpoch.value + 1;
  context.requestEpoch.value = epoch;
  context.currentAgentId.value = ownerId;
  context.currentOwnerKey.value = ownerKey;
  return { ownerKey, epoch };
}

function isCurrentTopicRequest(
  context: TopicStoreContext,
  ownerKey: string,
  epoch: number,
): boolean {
  return (
    context.currentOwnerKey.value === ownerKey &&
    context.requestEpoch.value === epoch
  );
}

const loadTopicList = async (
  context: TopicStoreContext,
  ownerId: string,
  owner_type: string,
) => {
  if (!ownerId) return;
  if (owner_type !== "agent" && owner_type !== "group") {
    throw new Error(`不支持的话题所有者类型：${owner_type}`);
  }
  const ownerType: "agent" | "group" = owner_type;
  const request = beginTopicRequest(context, ownerId, ownerType);
  const { ownerKey: requestOwnerKey, epoch: requestEpoch } = request;
  console.log(`[TopicStore] 正在加载 ${ownerType} 所有者的话题：${ownerId}`);
  context.loading.value = true;

  try {
    const channel = new Channel<Topic[]>();
    context.topics.value = [];
    channel.onmessage = (chunk) => {
      if (!isCurrentTopicRequest(context, requestOwnerKey, requestEpoch)) return;
      context.topics.value.push(...mapTopicChunk(chunk, ownerId, ownerType));
      context.topics.value = [...context.topics.value];
    };
    await invoke("get_topics_streamed", {
      ownerId,
      ownerType,
      onChunk: channel,
    });
    console.log(`[TopicStore] 所有者 ${ownerId} 的话题列表流式加载完成`);
  } catch (error) {
    console.error("[TopicStore] 加载话题失败：", error);
  } finally {
    if (isCurrentTopicRequest(context, requestOwnerKey, requestEpoch)) {
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
  const createEpoch = context.requestEpoch.value;
  const createOwnerKey = `${ownerType}:${ownerId}`;
  try {
    console.log(
      `[TopicStore] 正在为 ${ownerType} ${ownerId} 创建话题“${name}”`,
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
    if (isCurrentTopicRequest(context, createOwnerKey, createEpoch)) {
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
    console.error("[TopicStore] 创建话题失败：", error);
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
    console.log(`[TopicStore] 正在删除话题：${topicId}`);
    await invoke("delete_topic", { ownerId, ownerType, topicId });
    context.cancelUnreadReceiptsForTopic?.({ ownerId, ownerType, topicId });
    context.topics.value = context.topics.value.filter(
      (topic) => !hasTopicIdentity(topic, ownerId, ownerType, topicId),
    );
    context.notificationStore.addNotification({
      type: "success",
      title: "话题删除成功",
      message: "话题及其记录已被移除",
      toastOnly: true,
    });
    if (context.refreshOwnerUnreadCount) {
      try {
        await context.refreshOwnerUnreadCount(ownerId, ownerType);
      } catch (error) {
        console.error("[TopicStore] 删除话题后刷新 owner 未读聚合失败：", error);
      }
    }
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
    console.error("[TopicStore] 删除话题失败：", error);
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
      `[TopicStore] 正在将话题 ${topicId} 的标题更新为“${newTitle}”`,
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
    console.error("[TopicStore] 更新话题标题失败：", error);
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
    console.error("[TopicStore] 切换话题锁定状态失败：", error);
    throw error;
  }
};

const setTopicUnread = async (
  context: TopicStoreContext,
  ownerId: string,
  ownerType: "agent" | "group",
  topicId: string,
  unread: boolean,
  queue: TopicUnreadMutationQueue,
) => {
  const identity = { ownerId, ownerType, topicId };
  return queue.enqueue(identity, async () => {
    try {
      const result = await invoke<TopicUnreadState>("set_topic_unread", {
        ...identity,
        unread,
      });
      const state = validateTopicUnreadState(result, identity);
      applyTopicUnreadState(context.topics, state);
      if (
        context.applyOwnerUnreadCount &&
        state.ownerUnreadCount !== undefined
      ) {
        context.applyOwnerUnreadCount(
          state.ownerId,
          state.ownerType,
          state.ownerUnreadCount,
        );
      }
    } catch (error) {
      await reportTopicUnreadMutationFailure(identity, error, {
        notificationStore: context.notificationStore,
        reloadOwnerTopics: context.reloadOwnerTopics,
        applyOwnerUnreadCount: context.applyOwnerUnreadCount,
        refreshOwnerUnreadCount: context.refreshOwnerUnreadCount,
      });
    }
  });
};

const invalidateAllTopicCaches = (context: TopicStoreContext) => {
  context.requestEpoch.value += 1;
  context.topics.value = [];
  context.loading.value = false;
  console.log("[TopicStore] 已使所有话题缓存失效");
};

export function createTopicListActions(context: TopicStoreContext) {
  const unreadMutationQueue =
    context.unreadMutationQueue ?? createTopicUnreadMutationQueue();
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
    ) =>
      setTopicUnread(
        context,
        ownerId,
        ownerType,
        topicId,
        unread,
        unreadMutationQueue,
      ),
    invalidateAllTopicCaches: () => invalidateAllTopicCaches(context),
    setUnreadReceiptCancellation: (
      callback: (identity: ConversationIdentity) => void,
    ) => {
      context.cancelUnreadReceiptsForTopic = callback;
    },
  };
}
