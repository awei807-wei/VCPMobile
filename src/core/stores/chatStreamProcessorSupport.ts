import type { ChatMessage, TailFrame } from "../types/chat";
import type { ConversationIdentity } from "./chatStoreIdentity";

export type ReactiveMessageMap = Map<string, ChatMessage>;

export interface RafUpdate {
  content: string | null;
  blocks: any[] | null;
  tailContent: string | null;
  tailBlock: any | null;
  tailFrame: TailFrame | null;
  tailSnapshot: any[] | null;
  animationFrameId: number | null;
  lastRenderTime: number;
}

export interface StreamState {
  activeStreamMessages: ReactiveMessageMap;
  rAFPendingUpdates: Map<string, RafUpdate>;
  cleanupTimers: Set<ReturnType<typeof setTimeout>>;
  streamingMessageId: { value: string | null };
  streamingMessageKey: { value: string | null };
  streamGenerations: Map<string, number>;
  generationWatermarks?: Map<string, GenerationWatermark>;
  generationClocks?: Map<string, number>;
  wireGenerations?: Map<string, WireGenerationState>;
  /** Message identities whose unread receipt has been submitted in this runtime. */
  unreadMessageKeys?: Set<string>;
  /** Message identities with an unread receipt submission currently in flight. */
  unreadMessageInFlightKeys?: Set<string>;
  /** Delayed reconciliation attempts for failed unread receipt submissions. */
  unreadMessageRetryTimers?: Map<string, ReturnType<typeof setTimeout>>;
  /** Retry metadata for unread receipt submissions. */
  unreadMessageRetryStates?: Map<string, UnreadMessageRetryState>;
  /** Complete identities tracked by the unread receipt ledger. */
  unreadMessageReceiptRecords?: Map<string, UnreadMessageReceiptRecord>;
  /** Receipt keys that reached a terminal failure in this runtime. */
  unreadMessageFailedKeys?: Set<string>;
  /** Deleted message identities whose active stream may still emit frames. */
  unreadMessageReceiptTombstones?: Set<string>;
  /** Deleted topic identities whose active streams may still emit frames. */
  unreadTopicReceiptTombstones?: Set<string>;
  /** Monotonic token source used to invalidate stale promise callbacks. */
  unreadMessageReceiptSequence?: number;
}

export interface UnreadMessageReceiptRecord {
  identity: ConversationIdentity;
  messageId: string;
}

export interface UnreadMessageRetryState extends UnreadMessageReceiptRecord {
  attempt: number;
  startedAt: number;
  token: number;
}

export interface UnreadReceiptSubmissionResult {
  success: boolean;
  permanent?: boolean;
  cancelled?: boolean;
}

export type UnreadReceiptSubmission =
  | void
  | boolean
  | UnreadReceiptSubmissionResult
  | Promise<void | boolean | UnreadReceiptSubmissionResult>;

export type UnreadReceiptActiveGuard = () => boolean;

export interface GenerationWatermark {
  generation: number;
}

export interface WireGenerationState {
  wireGeneration: number;
  streamGeneration: number;
}

export function createGenerationWatermarks(): Map<string, GenerationWatermark> {
  return new Map<string, GenerationWatermark>();
}

export function nextStreamGeneration(
  state: StreamState,
  messageKey: string,
  minimum = 0,
): number {
  const previous = Math.max(
    state.generationClocks?.get(messageKey) || 0,
    state.generationWatermarks?.get(messageKey)?.generation || 0,
    state.streamGenerations.get(messageKey) || 0,
  );
  const generation = Math.max(previous + 1, minimum);
  state.generationClocks?.set(messageKey, generation);
  return generation;
}

export interface StreamProcessorDeps {
  state: StreamState;
  computeShell: (msg: {
    role: string;
    agentId?: string;
    name?: string;
  }) => ChatMessage["shell"];
  addSessionStream: (identity: ConversationIdentity, messageId: string) => void;
  removeSessionStream: (
    identity: ConversationIdentity,
    messageId: string,
  ) => void;
  incrementTopicMsgCount: (identity: ConversationIdentity) => void;
  incrementTopicUnreadCount: (
    identity: ConversationIdentity,
    messageId: string,
  ) => UnreadReceiptSubmission;
  incrementTopicUnreadCountWithGuard?: (
    identity: ConversationIdentity,
    messageId: string,
    isActive: UnreadReceiptActiveGuard,
  ) => UnreadReceiptSubmission;
  currentIdentity: () => ConversationIdentity | null;
  invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  isStreamDebugEnabled: () => boolean;
  recordStreamTrace: (data: unknown) => void;
  streamDebugLog: (...args: unknown[]) => void;
}

export interface ParsedStreamEvent {
  event: any;
  messageId: string;
  identity: ConversationIdentity;
  messageKey: string;
  topicKey: string;
  context: Record<string, any>;
  generation: number;
  wireGeneration: number;
}

export function clearStreamMessageRendering(
  state: StreamState,
  messageKey: string,
  forceFlush: boolean,
): void {
  const update = state.rAFPendingUpdates.get(messageKey);
  if (!update) return;
  if (update.animationFrameId !== null) {
    cancelAnimationFrame(update.animationFrameId);
    update.animationFrameId = null;
  }
  if (forceFlush) {
    const message = state.activeStreamMessages.get(messageKey);
    if (message) {
      if (update.content !== null) message.content = update.content;
      if (update.blocks !== null) message.blocks = update.blocks;
      if (update.tailContent !== null) message.tailContent = update.tailContent;
      if (update.tailBlock !== undefined) message.tailBlock = update.tailBlock;
      if (update.tailSnapshot !== null)
        message.tailSnapshot = update.tailSnapshot as any;
      if (update.tailFrame !== null) message.tailFrame = update.tailFrame;
    }
  }
  state.rAFPendingUpdates.delete(messageKey);
}

export { applyAuroraUpdate } from "./chatStreamProcessorRendering";
export {
  firstDefined,
  parseStreamEvent,
} from "./chatStreamProcessorParsing";

export function extractTextChunk(chunk: any): string {
  if (typeof chunk === "string") return chunk;
  if (!chunk || !Array.isArray(chunk.choices) || chunk.choices.length === 0)
    return "";
  const delta = chunk.choices[0]?.delta;
  return typeof delta?.content === "string" ? delta.content : "";
}

export function appendStreamError(message: ChatMessage, error: unknown): void {
  if (!error) return;
  const errorText = `\n\n> VCP流式错误: ${String(error)}`;
  const currentContent = message.content || "";
  if (!currentContent.endsWith(errorText))
    message.content = currentContent + errorText;
  message.finishReason = "error";
}

export function clearStreamRendering(state: StreamState): void {
  state.rAFPendingUpdates.forEach((update) => {
    if (update.animationFrameId !== null)
      cancelAnimationFrame(update.animationFrameId);
  });
  state.rAFPendingUpdates.clear();
  state.cleanupTimers.forEach(clearTimeout);
  state.cleanupTimers.clear();
  state.streamGenerations.clear();
  state.generationWatermarks?.clear();
  state.generationClocks?.clear();
  state.wireGenerations?.clear();
  state.unreadMessageKeys?.clear();
  state.unreadMessageInFlightKeys?.clear();
  state.unreadMessageRetryTimers?.forEach(clearTimeout);
  state.unreadMessageRetryTimers?.clear();
  state.unreadMessageRetryStates?.clear();
  state.unreadMessageReceiptRecords?.clear();
  state.unreadMessageFailedKeys?.clear();
  state.unreadMessageReceiptTombstones?.clear();
  state.unreadTopicReceiptTombstones?.clear();
}
