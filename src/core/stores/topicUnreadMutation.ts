import { invoke } from "@tauri-apps/api/core";
import type { Ref } from "vue";
import type { ConversationIdentity } from "./chatStoreIdentity";
import type {
  UnreadReceiptActiveGuard,
  UnreadReceiptSubmissionResult,
} from "./chatStreamProcessorSupport";
import type { Topic } from "./topicTypes";
import type {
  TopicSessionState,
  TopicUnreadMutationServices,
  TopicUnreadState,
} from "./topicListCounters";

export interface TopicUnreadMutationRunnerDeps {
  topics: Ref<Topic[]>;
  sessionStore: TopicSessionState;
  identity: ConversationIdentity;
  messageId: string;
  services: TopicUnreadMutationServices;
  isActive?: UnreadReceiptActiveGuard;
  validateState: (
    value: unknown,
    identity: ConversationIdentity,
  ) => TopicUnreadState;
  applyState: (
    topics: Ref<Topic[]>,
    state: TopicUnreadState,
    services: TopicUnreadMutationServices,
  ) => void;
  isPermanentError: (error: unknown) => boolean;
  reportFailure: (
    identity: ConversationIdentity,
    error: unknown,
    services: TopicUnreadMutationServices,
  ) => Promise<void>;
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

async function submitTopicUnreadMutation(
  deps: TopicUnreadMutationRunnerDeps,
): Promise<boolean> {
  if (deps.isActive && !deps.isActive()) return false;
  const result = await invoke<TopicUnreadState>(
    "increment_topic_unread_count",
    {
      ...deps.identity,
      msgId: deps.messageId,
      markUnread: !isCurrentTopic(deps.sessionStore, deps.identity),
    },
  );
  if (deps.isActive && !deps.isActive()) return false;
  deps.applyState(
    deps.topics,
    deps.validateState(result, deps.identity),
    deps.services,
  );
  return true;
}

async function reportAndRetry(
  deps: TopicUnreadMutationRunnerDeps,
  error: unknown,
): Promise<UnreadReceiptSubmissionResult> {
  await deps.reportFailure(deps.identity, error, deps.services);
  if (deps.isActive && !deps.isActive()) {
    return { success: false, cancelled: true };
  }
  try {
    if (!(await submitTopicUnreadMutation(deps))) {
      return { success: false, cancelled: true };
    }
    return { success: true };
  } catch (retryError) {
    if (deps.isActive && !deps.isActive()) {
      return { success: false, cancelled: true };
    }
    console.error("[TopicStore] 话题未读状态重试失败：", retryError);
    return {
      success: false,
      permanent: deps.isPermanentError(retryError),
    };
  }
}

export async function runTopicUnreadMutation(
  deps: TopicUnreadMutationRunnerDeps,
): Promise<UnreadReceiptSubmissionResult> {
  try {
    if (!(await submitTopicUnreadMutation(deps))) {
      return { success: false, cancelled: true };
    }
    return { success: true };
  } catch (error) {
    if (deps.isActive && !deps.isActive()) {
      return { success: false, cancelled: true };
    }
    if (deps.isPermanentError(error)) {
      await deps.reportFailure(deps.identity, error, deps.services);
      return { success: false, permanent: true };
    }
    return reportAndRetry(deps, error);
  }
}
