import { invoke } from "@tauri-apps/api/core";
import type { ComputedRef, Ref } from "vue";
import type { ChatMessage } from "../types/chat";
import {
  clearStreamMessageRendering,
  type StreamState,
} from "./chatStreamProcessor";
import {
  makeConversationIdentity,
  type ConversationIdentity,
  type ConversationOwnerType,
} from "./chatStoreIdentity";

interface StreamControlDeps {
  currentIdentity: () => ConversationIdentity | null;
  activeStreamingIds: ComputedRef<Set<string>>;
  activeStreamMessages: Map<string, ChatMessage>;
  runtimeState: StreamState;
  streamingMessageId: Ref<string | null>;
  streamingMessageKey: Ref<string | null>;
  removeSessionStream: (
    identity: ConversationIdentity,
    messageId: string,
  ) => void;
  activeMessageKey: (
    identity: ConversationIdentity,
    messageId: string,
  ) => string | null;
}

type StopMessageCallback = (messageId: string) => Promise<void>;

async function stopMessage(
  deps: StreamControlDeps,
  ownerIdOrMessageId: string,
  ownerTypeOrCallback?: ConversationOwnerType | StopMessageCallback,
  topicId?: string,
  messageId?: string,
  onUpdateMessage?: StopMessageCallback,
): Promise<void> {
  const explicit =
    typeof ownerTypeOrCallback === "string" && topicId && messageId
      ? makeConversationIdentity(
          ownerIdOrMessageId,
          ownerTypeOrCallback,
          topicId,
        )
      : null;
  const identity = explicit || deps.currentIdentity();
  const resolvedMessageId = explicit ? messageId! : ownerIdOrMessageId;
  const callback = explicit
    ? onUpdateMessage
    : typeof ownerTypeOrCallback === "function"
      ? ownerTypeOrCallback
      : undefined;
  if (!identity || !resolvedMessageId) return;

  const messageKey = deps.activeMessageKey(identity, resolvedMessageId);
  if (!messageKey) return;
  try {
    await invoke("interruptRequest", {
      messageId: resolvedMessageId,
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
    });
    const message = deps.activeStreamMessages.get(messageKey);
    if (message) {
      message.isThinking = false;
      message.finishReason = "interrupted";
      const errorText = "\n\n> VCP流式错误: 请求已中止";
      if (!(message.content || "").endsWith(errorText))
        message.content = (message.content || "") + errorText;
    }
    clearStreamMessageRendering(deps.runtimeState, messageKey, false);
    if (deps.streamingMessageKey.value === messageKey) {
      deps.streamingMessageId.value = null;
      deps.streamingMessageKey.value = null;
    }
    deps.removeSessionStream(identity, resolvedMessageId);
    await callback?.(resolvedMessageId);
  } catch (error) {
    console.error(
      `[ChatStreamStore] Failed to interrupt stream for ${messageKey}:`,
      error,
    );
  }
}

async function stopGroupTurn(
  deps: StreamControlDeps,
  topicId: string,
): Promise<void> {
  const identity = deps.currentIdentity();
  if (
    !identity ||
    identity.ownerType !== "group" ||
    identity.topicId !== topicId
  )
    return;
  try {
    await invoke("interruptGroupTurn", {
      groupId: identity.ownerId,
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId,
    });
    const activeIds = Array.from(deps.activeStreamingIds.value);
    await Promise.all(
      activeIds.map((id) =>
        stopMessage(deps, identity.ownerId, identity.ownerType, topicId, id),
      ),
    );
  } catch (error) {
    console.error("[ChatStreamStore] Failed to stop group turn:", error);
  }
}

export function createStreamControls(deps: StreamControlDeps) {
  return {
    stopMessage: (
      ownerIdOrMessageId: string,
      ownerTypeOrCallback?: ConversationOwnerType | StopMessageCallback,
      topicId?: string,
      messageId?: string,
      onUpdateMessage?: StopMessageCallback,
    ) =>
      stopMessage(
        deps,
        ownerIdOrMessageId,
        ownerTypeOrCallback,
        topicId,
        messageId,
        onUpdateMessage,
      ),
    stopGroupTurn: (topicId: string) => stopGroupTurn(deps, topicId),
  };
}
