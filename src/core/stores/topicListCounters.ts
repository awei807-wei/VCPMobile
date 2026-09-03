import type { Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { ConversationIdentity } from "./chatStoreIdentity";
import type { Topic } from "./topicTypes";

interface TopicSessionState {
  currentTopicId: string | null;
  currentSelectedItem: { id?: string; type?: string } | null;
}

function replaceTopic(
  topics: Ref<Topic[]>,
  index: number,
  update: Partial<Topic>,
): void {
  topics.value[index] = { ...topics.value[index], ...update };
  topics.value = [...topics.value];
}

function findTopicIndex(
  topics: Ref<Topic[]>,
  identity: ConversationIdentity,
): number {
  return topics.value.findIndex(
    (topic) =>
      topic.id === identity.topicId &&
      topic.ownerId === identity.ownerId &&
      topic.ownerType === identity.ownerType,
  );
}

function isCurrentTopic(
  sessionStore: TopicSessionState,
  identity: ConversationIdentity,
): boolean {
  return (
    sessionStore.currentTopicId === identity.topicId &&
    sessionStore.currentSelectedItem?.id === identity.ownerId &&
    sessionStore.currentSelectedItem?.type === identity.ownerType
  );
}

function incrementTopicMsgCount(
  topics: Ref<Topic[]>,
  identity: ConversationIdentity,
): void {
  const index = findTopicIndex(topics, identity);
  if (index === -1) return;
  const topic = topics.value[index];
  replaceTopic(topics, index, { msgCount: (topic.msgCount || 0) + 1 });
}

function incrementTopicUnreadCount(
  topics: Ref<Topic[]>,
  sessionStore: TopicSessionState,
  identity: ConversationIdentity,
): void {
  const index = findTopicIndex(topics, identity);
  if (index === -1 || isCurrentTopic(sessionStore, identity)) return;
  const topic = topics.value[index];
  replaceTopic(topics, index, {
    unreadCount: (topic.unreadCount || 0) + 1,
    unread: true,
  });
}

function decrementTopicMsgCount(
  topics: Ref<Topic[]>,
  identity: ConversationIdentity,
  count = 1,
): void {
  const index = findTopicIndex(topics, identity);
  if (index === -1) return;
  const topic = topics.value[index];
  replaceTopic(topics, index, {
    msgCount: Math.max(0, (topic.msgCount || 0) - count),
  });
}

function markTopicAsRead(
  topics: Ref<Topic[]>,
  identity: ConversationIdentity,
): void {
  const index = findTopicIndex(topics, identity);
  if (index === -1) return;

  const topic = topics.value[index];
  if (!topic.unread && !(topic.unreadCount && topic.unreadCount > 0)) return;
  replaceTopic(topics, index, { unread: false, unreadCount: 0 });
  void invoke("set_topic_unread", {
    ownerId: topic.ownerId,
    ownerType: topic.ownerType,
    topicId: identity.topicId,
    unread: false,
  }).catch(() => {});
}

export function useTopicListCounters(
  topics: Ref<Topic[]>,
  sessionStore: TopicSessionState,
) {
  return {
    incrementTopicMsgCount: (identity: ConversationIdentity) =>
      incrementTopicMsgCount(topics, identity),
    incrementTopicUnreadCount: (identity: ConversationIdentity) =>
      incrementTopicUnreadCount(topics, sessionStore, identity),
    decrementTopicMsgCount: (identity: ConversationIdentity, count = 1) =>
      decrementTopicMsgCount(topics, identity, count),
    markTopicAsRead: (identity: ConversationIdentity) =>
      markTopicAsRead(topics, identity),
  };
}
