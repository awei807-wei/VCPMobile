import type { ChatMessage } from "../types/chat";
import {
  applyAuroraUpdate,
  clearStreamMessageRendering,
  clearStreamRendering,
  extractTextChunk,
  firstDefined,
  nextStreamGeneration,
  parseStreamEvent,
  type GenerationWatermark,
  type ParsedStreamEvent,
  type StreamProcessorDeps,
  type StreamState,
} from "./chatStreamProcessorSupport";
import type { ConversationOwnerType } from "./chatStoreIdentity";
import {
  cancelUnreadReceipt,
  cancelUnreadReceiptsForTopic,
  releaseUnreadReceiptTombstonesAfterTerminal,
} from "./chatStreamUnreadReceiptCleanup";
import { submitUnreadReceipt } from "./chatStreamUnreadReceiptSubmission";

export interface StreamEventCallbacks {
  /** Recovery hydration/resume already has a persisted message row. */
  countMessage?: boolean;
  onMessageCreated?: (
    message: ChatMessage,
    topicId: string,
    ownerId: string,
    ownerType: ConversationOwnerType,
    generation: number,
  ) => void;
  onStreamFinished?: (
    messageId: string,
    topicId: string,
    ownerId: string,
    ownerType: ConversationOwnerType,
    generation: number,
  ) => void;
}

async function finalizeMessage(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  message: ChatMessage,
  callbacks?: StreamEventCallbacks,
  shouldNotifyFinished: () => boolean = () => true,
): Promise<void> {
  const event = parsed.event;
  const content = terminalContent(message.content || "", event);
  const timestamp = event.timestamp || message.timestamp;
  let compiledBlocks: unknown;
  try {
    if (event.blocks) {
      compiledBlocks = event.blocks;
    } else {
      compiledBlocks = await deps.invoke("process_message_content", {
        content,
      });
    }
  } catch (error) {
    console.error("[ChatStreamStore] process_message_content failed:", error);
  }
  if (!shouldNotifyFinished() ||
      deps.state.activeStreamMessages.get(parsed.messageKey) !== message) {
    return;
  }
  message.content = content;
  message.isThinking = false;
  message.timestamp = timestamp;
  if (event.finishReason) message.finishReason = event.finishReason;
  if (event.type === "error") message.finishReason = "error";
  if (compiledBlocks !== undefined) message.blocks = compiledBlocks as any;
  message.tailContent = "";
  message.tailBlock = undefined;
  if (deps.isStreamDebugEnabled()) {
    console.log(
      `[VCP Stream Debugger] 流式传输结束！当前录制帧数: ${(window as any).__VCP_STREAM_TRACES__?.length || 0}`,
    );
  }
  callbacks?.onStreamFinished?.(
    parsed.messageId,
    parsed.identity.topicId,
    parsed.identity.ownerId,
    parsed.identity.ownerType,
    parsed.generation,
  );
}

function terminalContent(content: string, event: any): string {
  if (event.type !== "error" || !event.error) return content;
  const errorText = `\n\n> VCP流式错误: ${String(event.error)}`;
  return content.endsWith(errorText) ? content : content + errorText;
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
    generation: parsed.generation,
    shell: deps.computeShell({ role: "assistant", agentId, name: agentName }),
  };
}

function createOrGetMessage(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  generationDecision: StreamGenerationDecision,
  callbacks?: StreamEventCallbacks,
): ChatMessage {
  const existing = deps.state.activeStreamMessages.get(parsed.messageKey);
  if (existing) {
    submitUnreadReceipt(deps, parsed);
    return existing;
  }

  const message = createSkeleton(deps, parsed);
  deps.state.activeStreamMessages.set(parsed.messageKey, message);
  if (generationDecision === "new" && callbacks?.countMessage !== false) {
    deps.incrementTopicMsgCount(parsed.identity);
  }
  // The backend receipt is idempotent across replay, resume, hydration and
  // process recreation. A failed request remains retryable for a later event.
  submitUnreadReceipt(deps, parsed);
  callbacks?.onMessageCreated?.(
    message,
    parsed.identity.topicId,
    parsed.identity.ownerId,
    parsed.identity.ownerType,
    parsed.generation,
  );
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
  callbacks?: StreamEventCallbacks,
): Promise<void> {
  closeTerminalGeneration(deps, parsed);
  clearStreamMessageRendering(deps.state, parsed.messageKey, true);
  deps.removeSessionStream(parsed.identity, parsed.messageId);
  if (deps.state.streamingMessageKey.value === parsed.messageKey) {
    deps.state.streamingMessageId.value = null;
    deps.state.streamingMessageKey.value = null;
  }
  try {
    await finalizeMessage(deps, parsed, message, callbacks, () =>
      isCurrentTerminalGeneration(deps, parsed),
    );
  } finally {
    releaseUnreadReceiptTombstonesAfterTerminal(deps.state, parsed);
  }
}

function rememberGenerationWatermark(
  deps: StreamProcessorDeps,
  messageKey: string,
  generation: number,
): void {
  const watermarks = deps.state.generationWatermarks;
  if (!watermarks) return;
  const previous = watermarks.get(messageKey);
  if (!previous || generation > previous.generation)
    watermarks.set(messageKey, { generation });
}

function currentGenerationWatermark(
  deps: StreamProcessorDeps,
  messageKey: string,
): GenerationWatermark | undefined {
  const watermarks = deps.state.generationWatermarks;
  if (!watermarks) return undefined;
  return watermarks.get(messageKey);
}

function closeTerminalGeneration(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
): void {
  rememberGenerationWatermark(deps, parsed.messageKey, parsed.generation);
  if (deps.state.streamGenerations.get(parsed.messageKey) === parsed.generation)
    deps.state.streamGenerations.delete(parsed.messageKey);
}

function isCurrentTerminalGeneration(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
): boolean {
  const watermark = currentGenerationWatermark(deps, parsed.messageKey);
  const active = deps.state.streamGenerations.get(parsed.messageKey);
  return (
    watermark?.generation === parsed.generation &&
    (active === undefined || active === parsed.generation)
  );
}

type StreamGenerationDecision = "new" | "existing" | "replacement" | "recovery";

function acceptStreamGeneration(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
): StreamGenerationDecision | null {
  if (deps.state.wireGenerations) {
    const wireState = deps.state.wireGenerations?.get(parsed.messageKey);
    if (wireState && parsed.wireGeneration < wireState.wireGeneration)
      return null;
    if (wireState && parsed.wireGeneration === wireState.wireGeneration) {
      parsed.generation = wireState.streamGeneration;
    } else {
      parsed.generation = nextStreamGeneration(
        deps.state,
        parsed.messageKey,
        parsed.wireGeneration,
      );
      deps.state.wireGenerations?.set(parsed.messageKey, {
        wireGeneration: parsed.wireGeneration,
        streamGeneration: parsed.generation,
      });
    }
  }
  const generations = deps.state.streamGenerations;
  const watermark = currentGenerationWatermark(deps, parsed.messageKey);
  if (watermark && parsed.generation <= watermark.generation) return null;
  const previous = generations.get(parsed.messageKey);
  if (previous !== undefined && parsed.generation < previous) return null;
  if (previous !== undefined && parsed.generation === previous) return "existing";
  let decision: StreamGenerationDecision = "new";
  if (
    previous !== undefined ||
    (watermark && parsed.generation > watermark.generation)
  ) {
    clearStreamMessageRendering(deps.state, parsed.messageKey, false);
    deps.state.activeStreamMessages.delete(parsed.messageKey);
    deps.removeSessionStream(parsed.identity, parsed.messageId);
    if (deps.state.streamingMessageKey.value === parsed.messageKey) {
      deps.state.streamingMessageId.value = null;
      deps.state.streamingMessageKey.value = null;
    }
    decision = "replacement";
  }
  generations.set(parsed.messageKey, parsed.generation);
  return decision;
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
  const generationDecision = acceptStreamGeneration(deps, parsed);
  if (!generationDecision) return;
  const message = createOrGetMessage(deps, parsed, generationDecision, callbacks);
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
      await handleTerminalEvent(deps, parsed, message, callbacks);
      break;
    default:
      break;
  }
}

export function createStreamEventProcessor(deps: StreamProcessorDeps) {
  return (event: any, callbacks?: StreamEventCallbacks) =>
    processStreamEvent(deps, event, callbacks);
}

export {
  cancelUnreadReceipt,
  cancelUnreadReceiptsForTopic,
  clearStreamMessageRendering,
  clearStreamRendering,
};
export type { StreamState };
