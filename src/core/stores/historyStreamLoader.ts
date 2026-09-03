import { Channel, invoke } from "@tauri-apps/api/core";
import type { ChatMessage, HistoryChunk } from "../types/chat";
import type { HistoryLoaderDeps } from "./chatHistoryLoader";
import {
  isActiveHistoryRequest,
  type HistoryLoaderState,
} from "./chatHistoryLoaderSupport";
import type { ConversationIdentity } from "./chatStoreIdentity";
import {
  createHistoryStreamSettlement,
  type HistoryStreamSettlement,
  type HistoryStreamSettlementReason,
} from "./historyStreamSettlement";

interface HistoryStreamRequest {
  deps: HistoryLoaderDeps;
  state: HistoryLoaderState;
  identity: ConversationIdentity;
  limit: number;
  offset: number;
  requestEpoch: number;
  signal: AbortSignal;
}

function appendHistoryChunk(
  request: HistoryStreamRequest,
  buffer: ChatMessage[],
  chunk: HistoryChunk,
  settle: (reason: HistoryStreamSettlementReason) => void,
): void {
  const { deps, state, identity, requestEpoch, signal, limit } = request;
  if (!isActiveHistoryRequest(deps, state, identity, requestEpoch, signal)) {
    return;
  }
  const activeMessage = deps.streamStore.getActiveStreamMessage(
    identity.ownerId,
    identity.ownerType,
    identity.topicId,
    chunk.message.id,
  );
  buffer.push(activeMessage ?? chunk.message);
  if (!chunk.is_last) return;
  deps.currentChatHistory.value = [...buffer, ...deps.currentChatHistory.value];
  deps.historyOffset.value += buffer.length;
  if (buffer.length < limit) deps.hasMoreHistory.value = false;
  settle("final");
}

function invokeHistoryStream(
  request: HistoryStreamRequest,
  channel: Channel<HistoryChunk>,
): Promise<number> {
  const { identity, limit, offset } = request;
  return invoke<number>("load_chat_history_streamed", {
    ownerId: identity.ownerId,
    ownerType: identity.ownerType,
    topicId: identity.topicId,
    limit,
    offset,
    onMessage: channel,
  });
}

async function awaitStreamSettlement(
  invocation: Promise<number>,
  settlement: HistoryStreamSettlement,
  hasMoreHistory: HistoryLoaderDeps["hasMoreHistory"],
): Promise<void> {
  const invocationOutcome = invocation.then((total) => ({
    kind: "invocation" as const,
    total,
  }));
  const completionOutcome = settlement.completion.then((reason) => ({
    kind: "completion" as const,
    reason,
  }));
  const outcome = await Promise.race([invocationOutcome, completionOutcome]);
  if (outcome.kind === "completion") return;
  if (outcome.total === 0) {
    hasMoreHistory.value = false;
    settlement.settle("empty");
    return;
  }
  await settlement.completion;
}

/** 加载一页 Channel 历史，并保证 abort、超时和终帧都能结算。 */
export async function loadStreamedHistory(
  request: HistoryStreamRequest,
): Promise<void> {
  if (request.signal.aborted) return;
  const channel = new Channel<HistoryChunk>();
  const buffer: ChatMessage[] = [];
  const settlement = createHistoryStreamSettlement(request.signal, () => {
    channel.onmessage = () => undefined;
  });
  channel.onmessage = (chunk) =>
    appendHistoryChunk(request, buffer, chunk, settlement.settle);

  try {
    await awaitStreamSettlement(
      invokeHistoryStream(request, channel),
      settlement,
      request.deps.hasMoreHistory,
    );
    const { deps, state, identity, requestEpoch, signal } = request;
    if (!isActiveHistoryRequest(deps, state, identity, requestEpoch, signal)) {
      return;
    }
    buffer.forEach((message) =>
      deps.attachmentStore.resolveMessageAssets(message),
    );
  } finally {
    settlement.cleanup();
  }
}
