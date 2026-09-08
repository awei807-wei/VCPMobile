import type { TailFrame } from "../types/chat";
import type {
  ParsedStreamEvent,
  RafUpdate,
  StreamProcessorDeps,
} from "./chatStreamProcessorSupport";

const MAX_PENDING_TAIL_MUTATIONS = 512;

function mergeTailFrame(
  existing: TailFrame | null,
  incoming: TailFrame,
  latestSnapshot?: any[],
  forceSnapshot = false,
): TailFrame {
  const incomingMutations = incoming.mutations || [];
  const snapshotFrame = (): TailFrame => ({
    ...incoming,
    reset: true,
    snapshot: latestSnapshot
      ? [...latestSnapshot]
      : incoming.snapshot
        ? [...incoming.snapshot]
        : undefined,
    mutations: [],
  });

  if (
    existing &&
    incoming.epoch === existing.epoch &&
    incoming.frameSeq <= existing.frameSeq
  )
    return existing;
  if (forceSnapshot || existing?.reset) return snapshotFrame();
  if (
    existing &&
    incoming.epoch === existing.epoch &&
    incoming.frameSeq > existing.frameSeq + 1
  )
    return snapshotFrame();
  if (!existing || incoming.reset || incoming.epoch !== existing.epoch) {
    return {
      ...incoming,
      mutations: incoming.reset ? [] : [...incomingMutations],
      snapshot: incoming.snapshot ? [...incoming.snapshot] : undefined,
    };
  }
  const mutations = [
    ...(existing.reset ? [] : existing.mutations || []),
    ...incomingMutations,
  ];
  if (mutations.length > MAX_PENDING_TAIL_MUTATIONS) return snapshotFrame();
  return {
    ...incoming,
    reset: existing.reset || incoming.reset,
    snapshot: incoming.snapshot || existing.snapshot,
    mutations,
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
  if (typeof aurora.content === "string") {
    update.content = aurora.content;
  } else if (typeof aurora.chunk === "string" && aurora.chunk.length > 0) {
    const currentContent =
      update.content !== null
        ? update.content
        : state.activeStreamMessages.get(parsed.messageKey)?.content || "";
    update.content = currentContent + aurora.chunk;
  }
  if (aurora.stableChanged && aurora.stableBlocks)
    update.blocks = aurora.stableBlocks;
  if (aurora.tailFrame) {
    deps.streamDebugLog(
      `[chatStreamStore] Received tailFrame seq=${aurora.tailFrame.frameSeq} mutations=${aurora.tailFrame.mutations?.length || 0} for ${parsed.messageKey}`,
    );
    const latestSnapshot =
      aurora.tailFrame.snapshot || aurora.tailSnapshot || aurora.tailBlock?.nodes;
    update.tailFrame = mergeTailFrame(
      update.tailFrame ||
        deps.state.activeStreamMessages.get(parsed.messageKey)?.tailFrame ||
        null,
      aurora.tailFrame,
      latestSnapshot,
      typeof document !== "undefined" && document.hidden,
    );
    if (latestSnapshot) update.tailSnapshot = latestSnapshot as any[];
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
