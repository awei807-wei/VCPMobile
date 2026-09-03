import type { ChatMessage, TailFrame } from "../types/chat";
import {
  makeConversationIdentity,
  makeMessageIdentity,
  messageIdentityKey,
  topicIdentityKey,
  type ConversationIdentity,
  type ConversationOwnerType,
} from "./chatStoreIdentity";

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
  incrementTopicUnreadCount: (identity: ConversationIdentity) => void;
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
}

export function firstDefined(...values: unknown[]): unknown {
  return values.find((value) => value !== undefined && value !== null);
}

function sameOptionalField(left: unknown, right: unknown): boolean {
  return (
    left === undefined ||
    left === null ||
    right === undefined ||
    right === null ||
    left === right
  );
}

interface StreamEventFields {
  context: Record<string, any>;
  messageId: string;
  topicId: string;
}

function readStreamEventFields(event: any): StreamEventFields | null {
  if (!event || typeof event !== "object") return null;
  const context =
    event.context && typeof event.context === "object"
      ? (event.context as Record<string, any>)
      : {};
  const messageId = firstDefined(event.messageId, event.message_id);
  const topicId = firstDefined(
    context.topicId,
    context.topic_id,
    event.topicId,
    event.topic_id,
  );
  if (
    typeof messageId !== "string" ||
    !messageId ||
    typeof topicId !== "string" ||
    !topicId
  )
    return null;
  if (
    !sameOptionalField(event.messageId, event.message_id) ||
    !sameOptionalField(context.topicId, context.topic_id) ||
    !sameOptionalField(event.topicId, event.topic_id)
  )
    return null;
  return { context, messageId, topicId };
}

interface StreamOwnerFields {
  explicitOwnerType: unknown;
  groupId: unknown;
  agentId: unknown;
  ownerId: unknown;
  groupFlag: boolean;
  explicitIsAgent: boolean;
}

function readStreamOwnerFields(
  event: any,
  context: Record<string, any>,
): StreamOwnerFields | null {
  if (
    !sameOptionalField(context.ownerType, context.owner_type) ||
    !sameOptionalField(event.ownerType, event.owner_type) ||
    !sameOptionalField(context.ownerType, event.ownerType)
  )
    return null;
  return {
    explicitOwnerType: firstDefined(
      context.ownerType,
      context.owner_type,
      event.ownerType,
      event.owner_type,
    ),
    groupId: firstDefined(context.groupId, context.group_id),
    agentId: firstDefined(context.agentId, context.agent_id),
    ownerId: firstDefined(
      context.ownerId,
      context.owner_id,
      event.ownerId,
      event.owner_id,
    ),
    groupFlag:
      context.isGroupMessage === true || context.is_group_message === true,
    explicitIsAgent:
      context.isGroupMessage === false || context.is_group_message === false,
  };
}

interface ResolvedStreamOwner {
  ownerType: ConversationOwnerType;
  ownerId: string;
}

function resolveStreamOwner(
  fields: StreamOwnerFields,
): ResolvedStreamOwner | null {
  const {
    explicitOwnerType,
    groupId,
    agentId,
    ownerId,
    groupFlag,
    explicitIsAgent,
  } = fields;
  if (
    explicitOwnerType !== undefined &&
    explicitOwnerType !== "agent" &&
    explicitOwnerType !== "group"
  )
    return null;
  if (explicitIsAgent && groupId !== undefined) return null;

  let ownerType: ConversationOwnerType;
  if (explicitOwnerType === "group" || groupFlag || groupId !== undefined)
    ownerType = "group";
  else if (explicitOwnerType === "agent" || agentId !== undefined)
    ownerType = "agent";
  else return null;
  if (explicitOwnerType && explicitOwnerType !== ownerType) return null;

  const resolvedOwnerId =
    ownerType === "group"
      ? firstDefined(groupId, ownerId)
      : firstDefined(agentId, ownerId);
  if (typeof resolvedOwnerId !== "string" || !resolvedOwnerId) return null;
  if (!sameOptionalField(ownerId, resolvedOwnerId)) return null;
  return { ownerType, ownerId: resolvedOwnerId };
}

function buildParsedStreamEvent(
  event: any,
  fields: StreamEventFields,
  owner: ResolvedStreamOwner,
): ParsedStreamEvent | null {
  const identity = makeConversationIdentity(
    owner.ownerId,
    owner.ownerType,
    fields.topicId,
  );
  const messageIdentity = makeMessageIdentity(
    owner.ownerId,
    owner.ownerType,
    fields.topicId,
    fields.messageId,
  );
  if (!identity || !messageIdentity) return null;
  return {
    event,
    messageId: fields.messageId,
    identity,
    messageKey: messageIdentityKey(messageIdentity),
    topicKey: topicIdentityKey(identity),
    context: fields.context,
  };
}

/**
 * Resolve the owner namespace carried by legacy and Wire 1.4 stream events.
 * Incomplete or contradictory events are intentionally dropped: choosing an
 * Agent namespace here would let a Group response contaminate an Agent topic.
 */
export function parseStreamEvent(event: any): ParsedStreamEvent | null {
  const fields = readStreamEventFields(event);
  if (!fields) return null;
  const ownerFields = readStreamOwnerFields(event, fields.context);
  if (!ownerFields) return null;
  const owner = resolveStreamOwner(ownerFields);
  return owner ? buildParsedStreamEvent(event, fields, owner) : null;
}

function mergeTailFrame(
  existing: TailFrame | null,
  incoming: TailFrame,
): TailFrame {
  const incomingMutations = incoming.mutations || [];
  if (!existing || incoming.reset || incoming.epoch !== existing.epoch) {
    return {
      ...incoming,
      mutations: incoming.reset ? [] : [...incomingMutations],
      snapshot: incoming.snapshot ? [...incoming.snapshot] : undefined,
    };
  }
  return {
    ...incoming,
    reset: existing.reset || incoming.reset,
    snapshot: incoming.snapshot || existing.snapshot,
    mutations: [
      ...(existing.reset ? [] : existing.mutations || []),
      ...incomingMutations,
    ],
  };
}

function emptyRafUpdate(): RafUpdate {
  return {
    content: null,
    blocks: null,
    tailContent: null,
    tailBlock: null,
    tailFrame: null,
    tailSnapshot: null,
    animationFrameId: null,
    lastRenderTime: 0,
  };
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

function scheduleAuroraRender(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  aurora: any,
): void {
  const { state } = deps;
  let update = state.rAFPendingUpdates.get(parsed.messageKey);
  if (!update) {
    update = emptyRafUpdate();
    state.rAFPendingUpdates.set(parsed.messageKey, update);
  }
  if (typeof aurora.content === "string") update.content = aurora.content;
  if (aurora.stableChanged && aurora.stableBlocks)
    update.blocks = aurora.stableBlocks;
  if (aurora.tailFrame) {
    deps.streamDebugLog(
      `[chatStreamStore] Received tailFrame seq=${aurora.tailFrame.frameSeq} mutations=${aurora.tailFrame.mutations?.length || 0} for ${parsed.messageKey}`,
    );
    update.tailFrame = mergeTailFrame(update.tailFrame, aurora.tailFrame);
    if (aurora.tailFrame.snapshot)
      update.tailSnapshot = aurora.tailFrame.snapshot as any[];
  }
  if (aurora.tailSnapshot) update.tailSnapshot = aurora.tailSnapshot as any[];
  if (aurora.tailChanged) {
    update.tailContent = aurora.tail || "";
    update.tailBlock = aurora.tailBlock || null;
  }
  if (update.animationFrameId === null) {
    update.animationFrameId = requestAnimationFrame(() =>
      renderAuroraFrame(deps, parsed.messageKey),
    );
  }
}

function renderAuroraFrame(
  deps: StreamProcessorDeps,
  messageKey: string,
): void {
  const update = deps.state.rAFPendingUpdates.get(messageKey);
  if (!update) return;
  const now = performance.now();
  if (now - update.lastRenderTime < 33.3) {
    update.animationFrameId = requestAnimationFrame(() =>
      renderAuroraFrame(deps, messageKey),
    );
    return;
  }
  const message = deps.state.activeStreamMessages.get(messageKey);
  if (message) {
    if (update.content !== null) message.content = update.content;
    if (update.blocks !== null) message.blocks = update.blocks;
    if (update.tailSnapshot !== null)
      message.tailSnapshot = update.tailSnapshot as any;
    if (update.tailFrame !== null) message.tailFrame = update.tailFrame;
    if (update.tailContent !== null) message.tailContent = update.tailContent;
    if (update.tailBlock !== undefined) message.tailBlock = update.tailBlock;
  }
  update.lastRenderTime = now;
  update.content = null;
  update.blocks = null;
  update.tailContent = null;
  update.tailBlock = null;
  update.tailFrame = null;
  update.tailSnapshot = null;
  update.animationFrameId = null;
}

export function applyAuroraUpdate(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  aurora: any,
): void {
  scheduleAuroraRender(deps, parsed, aurora);
}

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
}
