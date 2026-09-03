import type { ChatMessage } from "../types/chat";
import {
  appendStreamError,
  applyAuroraUpdate,
  clearStreamMessageRendering,
  clearStreamRendering,
  extractTextChunk,
  firstDefined,
  parseStreamEvent,
  type ParsedStreamEvent,
  type StreamProcessorDeps,
  type StreamState,
} from "./chatStreamProcessorSupport";
import {
  sameConversationIdentity,
  type ConversationOwnerType,
} from "./chatStoreIdentity";

export interface StreamEventCallbacks {
  onMessageCreated?: (
    message: ChatMessage,
    topicId: string,
    ownerId: string,
    ownerType: ConversationOwnerType,
  ) => void;
  onStreamFinished?: (
    messageId: string,
    topicId: string,
    ownerId: string,
    ownerType: ConversationOwnerType,
  ) => void;
}

async function finalizeMessage(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  message: ChatMessage,
  callbacks?: StreamEventCallbacks,
): Promise<void> {
  const event = parsed.event;
  message.isThinking = false;
  if (event.timestamp) message.timestamp = event.timestamp;
  try {
    if (event.blocks) {
      message.blocks = event.blocks as any;
    } else {
      const compiledBlocks = await deps.invoke("process_message_content", {
        content: message.content || "",
      });
      message.blocks = compiledBlocks as any;
    }
  } catch (error) {
    console.error("[ChatStreamStore] process_message_content failed:", error);
  } finally {
    message.tailContent = "";
    message.tailBlock = undefined;
    if (deps.isStreamDebugEnabled()) {
      console.log(
        `[VCP Stream Debugger] 流式传输结束！当前录制帧数: ${(window as any).__VCP_STREAM_TRACES__?.length || 0}`,
      );
    }
  }
  callbacks?.onStreamFinished?.(
    parsed.messageId,
    parsed.identity.topicId,
    parsed.identity.ownerId,
    parsed.identity.ownerType,
  );
}

function createSkeleton(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
): ChatMessage {
  const context = parsed.context;
  const event = parsed.event;
  const agentId = firstDefined(context.agentId, context.agent_id) as
    | string
    | undefined;
  const agentName = context.agentName || context.agent_name;
  return {
    id: parsed.messageId,
    role: "assistant",
    name: agentName,
    content: "",
    timestamp: Date.now(),
    isThinking: event.type === "thinking",
    agentId,
    groupId:
      parsed.identity.ownerType === "group"
        ? parsed.identity.ownerId
        : undefined,
    isGroupMessage: parsed.identity.ownerType === "group",
    topicId: parsed.identity.topicId,
    shell: deps.computeShell({ role: "assistant", agentId, name: agentName }),
  };
}

function persistSkeleton(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  message: ChatMessage,
): void {
  const context = parsed.context;
  const agentId = firstDefined(context.agentId, context.agent_id);
  void deps
    .invoke("append_single_message", {
      ownerId: parsed.identity.ownerId,
      ownerType: parsed.identity.ownerType,
      topicId: parsed.identity.topicId,
      message: {
        id: parsed.messageId,
        role: "assistant",
        name: message.name || null,
        content: "",
        timestamp: message.timestamp,
        isThinking: message.isThinking,
        is_thinking: message.isThinking,
        agentId: agentId || null,
        groupId:
          parsed.identity.ownerType === "group"
            ? parsed.identity.ownerId
            : null,
        topicId: parsed.identity.topicId,
        isGroupMessage: parsed.identity.ownerType === "group",
      },
    })
    .catch((error) => {
      console.error(
        "[ChatStreamStore] Failed to persist initial thinking skeleton:",
        error,
      );
    });
}

function createOrGetMessage(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  callbacks?: StreamEventCallbacks,
): ChatMessage {
  const existing = deps.state.activeStreamMessages.get(parsed.messageKey);
  if (existing) return existing;

  const message = createSkeleton(deps, parsed);
  deps.state.activeStreamMessages.set(parsed.messageKey, message);
  deps.incrementTopicMsgCount(parsed.identity);
  if (!sameConversationIdentity(parsed.identity, deps.currentIdentity()))
    deps.incrementTopicUnreadCount(parsed.identity);
  callbacks?.onMessageCreated?.(
    message,
    parsed.identity.topicId,
    parsed.identity.ownerId,
    parsed.identity.ownerType,
  );
  persistSkeleton(deps, parsed, message);
  return message;
}

function handleThinkingEvent(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  message: ChatMessage,
): void {
  message.isThinking = true;
  deps.addSessionStream(parsed.identity, parsed.messageId);
  if (!deps.state.streamingMessageId.value) {
    deps.state.streamingMessageId.value = parsed.messageId;
    deps.state.streamingMessageKey.value = parsed.messageKey;
  }
}

function handleDataEvent(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  message: ChatMessage,
  event: any,
): void {
  message.isThinking = false;
  deps.addSessionStream(parsed.identity, parsed.messageId);
  const textChunk = extractTextChunk(event.chunk);
  if (textChunk) {
    message.content = (message.content || "") + textChunk;
    message.tailContent = message.content;
  }
}

function recordAuroraDebug(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  message: ChatMessage,
  aurora: any,
): void {
  if (!deps.isStreamDebugEnabled()) return;
  deps.recordStreamTrace({
    messageId: parsed.messageKey,
    auroraPayload: {
      stableChanged: aurora.stableChanged,
      stableBlocksCount: aurora.stableBlocks?.length || 0,
      stableBlocksHashes:
        aurora.stableBlocks?.map((block: any) => block.hash) || [],
      tailChanged: aurora.tailChanged,
      tailContent: aurora.tail || "",
      tailBlockType: aurora.tailBlock?.type || null,
      tailFrame: aurora.tailFrame
        ? {
            epoch: aurora.tailFrame.epoch,
            revision: aurora.tailFrame.revision,
            frameSeq: aurora.tailFrame.frameSeq,
            reset: aurora.tailFrame.reset,
            mutationsCount: aurora.tailFrame.mutations?.length || 0,
            hasSnapshot: !!aurora.tailFrame.snapshot,
          }
        : null,
    },
    msgSnapshot: {
      contentLength: message.content?.length || 0,
      blocksCount: message.blocks?.length || 0,
      tailContentLength: message.tailContent?.length || 0,
    },
  });
}

function handleAuroraEvent(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  message: ChatMessage,
  event: any,
): void {
  if (event.aurora) {
    recordAuroraDebug(deps, parsed, message, event.aurora);
    applyAuroraUpdate(deps, parsed, event.aurora);
  }
  message.isThinking = false;
  deps.addSessionStream(parsed.identity, parsed.messageId);
}

async function handleTerminalEvent(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  message: ChatMessage,
  event: any,
  callbacks?: StreamEventCallbacks,
): Promise<void> {
  clearStreamMessageRendering(deps.state, parsed.messageKey, true);
  if (event.finishReason) message.finishReason = event.finishReason;
  if (event.type === "error") appendStreamError(message, event.error);
  deps.removeSessionStream(parsed.identity, parsed.messageId);
  if (deps.state.streamingMessageKey.value === parsed.messageKey) {
    deps.state.streamingMessageId.value = null;
    deps.state.streamingMessageKey.value = null;
  }
  await finalizeMessage(deps, parsed, message, callbacks);
}

async function processStreamEvent(
  deps: StreamProcessorDeps,
  event: any,
  callbacks?: StreamEventCallbacks,
): Promise<void> {
  if (!["thinking", "data", "aurora", "end", "error"].includes(event?.type))
    return;
  const parsed = parseStreamEvent(event);
  if (!parsed) return;
  const message = createOrGetMessage(deps, parsed, callbacks);
  switch (event.type) {
    case "thinking":
      handleThinkingEvent(deps, parsed, message);
      break;
    case "data":
      handleDataEvent(deps, parsed, message, event);
      break;
    case "aurora":
      handleAuroraEvent(deps, parsed, message, event);
      break;
    case "end":
    case "error":
      await handleTerminalEvent(deps, parsed, message, event, callbacks);
      break;
    default:
      break;
  }
}

export function createStreamEventProcessor(deps: StreamProcessorDeps) {
  return (event: any, callbacks?: StreamEventCallbacks) =>
    processStreamEvent(deps, event, callbacks);
}

export { clearStreamMessageRendering, clearStreamRendering };
export type { StreamState };
