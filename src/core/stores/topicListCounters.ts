import type { Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { VcpNotification } from "./notification";
import type { ConversationIdentity } from "./chatStoreIdentity";
import { topicIdentityKey } from "./chatStoreIdentity";
import type { Topic } from "./topicTypes";
import type {
  UnreadReceiptActiveGuard,
  UnreadReceiptSubmissionResult,
} from "./chatStreamProcessorSupport";
import { runTopicUnreadMutation } from "./topicUnreadMutation";

/** 后端在未读事务提交时返回的完整复合身份状态。 */
export interface TopicUnreadState {
  ownerId: string;
  ownerType: "agent" | "group";
  topicId: string;
  unread: boolean;
  unreadCount: number;
  /** Owner aggregate returned by the same backend transaction. */
  ownerUnreadCount?: number;
}

export interface OwnerUnreadState {
  ownerId: string;
  ownerType: "agent" | "group";
  unreadCount: number;
}

export interface TopicSessionState {
  currentTopicId: string | null;
  currentSelectedItem: { id?: string; type?: string } | null;
}

interface NotificationSink {
  addNotification(payload: Partial<VcpNotification>): unknown;
}

export interface TopicUnreadMutationServices {
  notificationStore?: NotificationSink;
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
}

export interface TopicUnreadMutationQueue {
  enqueue<T>(
    identity: ConversationIdentity,
    operation: () => Promise<T>,
  ): Promise<T>;
  flush(): Promise<void>;
}

/** 为每个 owner 串行化未读状态写入，避免跨话题聚合旧结果覆盖新结果。 */
export function createTopicUnreadMutationQueue(): TopicUnreadMutationQueue {
  const tails = new Map<string, Promise<void>>();
  const enqueue = <T>(
    identity: ConversationIdentity,
    operation: () => Promise<T>,
  ): Promise<T> => {
    const key = `${identity.ownerType}:${encodeURIComponent(identity.ownerId)}`;
    const previous = tails.get(key) ?? Promise.resolve();
    const current = previous.then(operation, operation);
    const settled = current.then(
      () => undefined,
      () => undefined,
    );
    tails.set(key, settled);
    return current.finally(() => {
      if (tails.get(key) === settled) tails.delete(key);
    });
  };

  return {
    enqueue,
    async flush() {
      await Promise.all([...tails.values()]);
    },
  };
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

function errorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  if (error && typeof error === "object") {
    const value = error as {
      message?: unknown;
      error?: unknown;
      code?: unknown;
    };
    const parts = [value.message, value.error, value.code].filter(
      (part): part is string =>
        typeof part === "string" && part.trim().length > 0,
    );
    if (parts.length > 0) return parts.join(" ");
  }
  return "系统或网络异常，请重新加载话题列表";
}

export function isPermanentTopicUnreadMutationError(error: unknown): boolean {
  const message = errorMessage(error).toLowerCase();
  return (
    message.includes("not found") ||
    message.includes("does not exist") ||
    message.includes("no such") ||
    message.includes("不存在") ||
    message.includes("已删除") ||
    message.includes("deleted") ||
    message.includes("message_not_found") ||
    message.includes("topic_not_found") ||
    message.includes("entity_not_found")
  );
}

function applyOwnerUnreadState(
  state: TopicUnreadState,
  services: TopicUnreadMutationServices,
): void {
  if (
    services.applyOwnerUnreadCount &&
    typeof state.ownerUnreadCount === "number" &&
    Number.isSafeInteger(state.ownerUnreadCount) &&
    state.ownerUnreadCount >= -1
  ) {
    services.applyOwnerUnreadCount(
      state.ownerId,
      state.ownerType,
      state.ownerUnreadCount,
    );
  }
}

function applyMutationState(
  topics: Ref<Topic[]>,
  state: TopicUnreadState,
  services: TopicUnreadMutationServices,
): void {
  applyTopicUnreadState(topics, state);
  // This deliberately runs even when the topic is absent from the currently
  // loaded owner list: the owner badge is a separate authoritative aggregate.
  applyOwnerUnreadState(state, services);
}

export function validateTopicUnreadState(
  value: unknown,
  identity: ConversationIdentity,
): TopicUnreadState {
  if (!value || typeof value !== "object") {
    throw new Error("后端未返回有效的未读状态");
  }
  const state = value as Partial<TopicUnreadState>;
  if (
    state.ownerId !== identity.ownerId ||
    state.ownerType !== identity.ownerType ||
    state.topicId !== identity.topicId ||
    typeof state.unread !== "boolean" ||
    typeof state.unreadCount !== "number" ||
    !Number.isSafeInteger(state.unreadCount) ||
    state.unreadCount < 0 ||
    (state.ownerUnreadCount !== undefined &&
      (typeof state.ownerUnreadCount !== "number" ||
        !Number.isSafeInteger(state.ownerUnreadCount) ||
        state.ownerUnreadCount < -1))
  ) {
    throw new Error("后端返回的未读状态身份或计数无效");
  }
  return state as TopicUnreadState;
}

/** 只把同一复合身份的后端权威结果写回列表。 */
export function applyTopicUnreadState(
  topics: Ref<Topic[]>,
  state: TopicUnreadState,
): void {
  const index = findTopicIndex(topics, state);
  if (index === -1) return;
  replaceTopic(topics, index, {
    unread: state.unread,
    unreadCount: state.unreadCount,
  });
}

/** 记录失败、合并通知，并刷新仍处于当前 owner 下的话题列表。 */
export async function reportTopicUnreadMutationFailure(
  identity: ConversationIdentity,
  error: unknown,
  services: TopicUnreadMutationServices,
): Promise<void> {
  const message = errorMessage(error);
  console.error("[TopicStore] 话题未读状态同步失败：", error);
  services.notificationStore?.addNotification({
    id: `topic-unread-mutation:${topicIdentityKey(identity)}`,
    type: "error",
    title: "未读状态同步失败",
    message,
    duration: 5000,
  });
  await Promise.all([
    services.refreshOwnerUnreadCount
      ? services
          .refreshOwnerUnreadCount(identity.ownerId, identity.ownerType)
          .catch((refreshError) =>
            console.error(
              "[TopicStore] 刷新 owner 未读聚合失败：",
              refreshError,
            ),
          )
      : Promise.resolve(),
    services.reloadOwnerTopics
      ? services
          .reloadOwnerTopics(identity.ownerId, identity.ownerType)
          .catch((reloadError) =>
            console.error("[TopicStore] 刷新未读话题列表失败：", reloadError),
          )
      : Promise.resolve(),
  ]);
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

function setTopicMsgCount(
  topics: Ref<Topic[]>,
  identity: ConversationIdentity,
  msgCount: number,
): void {
  if (!Number.isSafeInteger(msgCount) || msgCount < 0) return;
  const index = findTopicIndex(topics, identity);
  if (index === -1) return;
  replaceTopic(topics, index, { msgCount });
}

let legacyMessageSequence = 0;

function incrementTopicUnreadCount(
  topics: Ref<Topic[]>,
  sessionStore: TopicSessionState,
  identity: ConversationIdentity,
  messageId: string | undefined,
  queue: TopicUnreadMutationQueue,
  services: TopicUnreadMutationServices,
  isActive?: UnreadReceiptActiveGuard,
): Promise<UnreadReceiptSubmissionResult> {
  return queue.enqueue(identity, () => {
    // Production stream callbacks provide the real id; this fallback keeps
    // the legacy manual counter API usable outside stream processing.
    const effectiveMessageId =
      messageId || `legacy-topic-event-${++legacyMessageSequence}`;
    return runTopicUnreadMutation({
      topics,
      sessionStore,
      identity,
      messageId: effectiveMessageId,
      services,
      isActive,
      validateState: validateTopicUnreadState,
      applyState: applyMutationState,
      isPermanentError: isPermanentTopicUnreadMutationError,
      reportFailure: reportTopicUnreadMutationFailure,
    });
  });
}

function markTopicAsRead(
  topics: Ref<Topic[]>,
  identity: ConversationIdentity,
  queue: TopicUnreadMutationQueue,
  services: TopicUnreadMutationServices,
): Promise<void> {
  return queue.enqueue(identity, async () => {
    try {
      const result = await invoke<TopicUnreadState>("set_topic_unread", {
        ...identity,
        unread: false,
      });
      applyMutationState(
        topics,
        validateTopicUnreadState(result, identity),
        services,
      );
    } catch (error) {
      await reportTopicUnreadMutationFailure(identity, error, services);
    }
  });
}

export function useTopicListCounters(
  topics: Ref<Topic[]>,
  sessionStore: TopicSessionState,
  services: TopicUnreadMutationServices & {
    queue?: TopicUnreadMutationQueue;
  } = {},
) {
  const queue = services.queue ?? createTopicUnreadMutationQueue();
  return {
    incrementTopicMsgCount: (identity: ConversationIdentity) =>
      incrementTopicMsgCount(topics, identity),
    setTopicMsgCount: (identity: ConversationIdentity, msgCount: number) =>
      setTopicMsgCount(topics, identity, msgCount),
    incrementTopicUnreadCount: (
      identity: ConversationIdentity,
      messageId?: string,
      isActive?: UnreadReceiptActiveGuard,
    ) =>
      incrementTopicUnreadCount(
        topics,
        sessionStore,
        identity,
        messageId,
        queue,
        services,
        isActive,
      ),
    markTopicAsRead: (identity: ConversationIdentity) =>
      markTopicAsRead(topics, identity, queue, services),
    flushUnreadMutations: () => queue.flush(),
  };
}
