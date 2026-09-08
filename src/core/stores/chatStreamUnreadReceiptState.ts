import type {
  ParsedStreamEvent,
  StreamProcessorDeps,
  StreamState,
  UnreadMessageReceiptRecord,
  UnreadMessageRetryState,
} from "./chatStreamProcessorSupport";
import { topicIdentityKey } from "./chatStoreIdentity";

export const UNREAD_RECEIPT_RETRY_INITIAL_DELAY_MS = 50;
export const UNREAD_RECEIPT_RETRY_MAX_DELAY_MS = 1_000;
export const UNREAD_RECEIPT_MAX_ATTEMPTS = 5;
export const UNREAD_RECEIPT_MAX_AGE_MS = 30_000;

export type UnreadReceiptSubmitter = (
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
) => void;

export function receiptErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object") {
    const value = error as { message?: unknown; error?: unknown; code?: unknown };
    const parts = [value.message, value.error, value.code].filter(
      (part): part is string => typeof part === "string",
    );
    if (parts.length > 0) return parts.join(" ");
  }
  return String(error ?? "");
}

export function isPermanentUnreadReceiptError(error: unknown): boolean {
  const message = receiptErrorMessage(error).toLowerCase();
  if (!message) return false;
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

export function getReceiptRecords(
  state: StreamState,
): Map<string, UnreadMessageReceiptRecord> {
  return (
    state.unreadMessageReceiptRecords ||
    (state.unreadMessageReceiptRecords = new Map())
  );
}

export function getRetryStates(
  state: StreamState,
): Map<string, UnreadMessageRetryState> {
  return (
    state.unreadMessageRetryStates ||
    (state.unreadMessageRetryStates = new Map())
  );
}

export function getFailedKeys(state: StreamState): Set<string> {
  return (
    state.unreadMessageFailedKeys ||
    (state.unreadMessageFailedKeys = new Set())
  );
}

export function getMessageTombstones(state: StreamState): Set<string> {
  return (
    state.unreadMessageReceiptTombstones ||
    (state.unreadMessageReceiptTombstones = new Set())
  );
}

export function getTopicTombstones(state: StreamState): Set<string> {
  return (
    state.unreadTopicReceiptTombstones ||
    (state.unreadTopicReceiptTombstones = new Set())
  );
}

export function isReceiptTombstoned(
  state: StreamState,
  parsed: Pick<ParsedStreamEvent, "messageKey" | "identity">,
): boolean {
  return !!(
    state.unreadMessageReceiptTombstones?.has(parsed.messageKey) ||
    state.unreadTopicReceiptTombstones?.has(topicIdentityKey(parsed.identity))
  );
}

export function isReceiptSubmissionActive(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  token: number,
): boolean {
  const retryState = deps.state.unreadMessageRetryStates?.get(
    parsed.messageKey,
  );
  return (
    retryState?.token === token && !isReceiptTombstoned(deps.state, parsed)
  );
}

export function rememberReceipt(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
): void {
  getReceiptRecords(deps.state).set(parsed.messageKey, {
    identity: parsed.identity,
    messageId: parsed.messageId,
  });
}

export function createRetryState(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
): UnreadMessageRetryState {
  const sequence = (deps.state.unreadMessageReceiptSequence || 0) + 1;
  deps.state.unreadMessageReceiptSequence = sequence;
  const state: UnreadMessageRetryState = {
    identity: parsed.identity,
    messageId: parsed.messageId,
    attempt: 0,
    startedAt: Date.now(),
    token: sequence,
  };
  getRetryStates(deps.state).set(parsed.messageKey, state);
  return state;
}

export function clearRetryState(
  deps: StreamProcessorDeps,
  messageKey: string,
): void {
  const timers = deps.state.unreadMessageRetryTimers;
  const timer = timers?.get(messageKey);
  if (timer !== undefined) clearTimeout(timer);
  timers?.delete(messageKey);
  deps.state.unreadMessageRetryStates?.delete(messageKey);
}

export function finishUnreadReceiptFailure(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  error: unknown,
): void {
  const retryState = getRetryStates(deps.state).get(parsed.messageKey);
  const attempts = retryState?.attempt || 0;
  getFailedKeys(deps.state).add(parsed.messageKey);
  clearRetryState(deps, parsed.messageKey);
  console.error("[ChatStreamStore] 未读收据同步已停止：", {
    messageKey: parsed.messageKey,
    topicKey: topicIdentityKey(parsed.identity),
    attempts,
    error,
  });
}

export function scheduleUnreadReceiptRetry(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  submit: UnreadReceiptSubmitter,
  error?: unknown,
): void {
  const completed = deps.state.unreadMessageKeys;
  if (completed?.has(parsed.messageKey)) return;
  if (getFailedKeys(deps.state).has(parsed.messageKey)) return;
  if (deps.state.unreadMessageInFlightKeys?.has(parsed.messageKey)) return;

  const retryState = getRetryStates(deps.state).get(parsed.messageKey);
  if (!retryState) return;
  const elapsed = Math.max(0, Date.now() - retryState.startedAt);
  if (
    retryState.attempt >= UNREAD_RECEIPT_MAX_ATTEMPTS ||
    elapsed >= UNREAD_RECEIPT_MAX_AGE_MS
  ) {
    finishUnreadReceiptFailure(deps, parsed, error);
    return;
  }

  const timers =
    deps.state.unreadMessageRetryTimers ||
    (deps.state.unreadMessageRetryTimers = new Map());
  if (timers.has(parsed.messageKey)) return;
  const backoff = Math.min(
    UNREAD_RECEIPT_RETRY_MAX_DELAY_MS,
    UNREAD_RECEIPT_RETRY_INITIAL_DELAY_MS * 2 ** Math.max(0, retryState.attempt - 1),
  );
  const delay = Math.min(
    backoff,
    Math.max(0, UNREAD_RECEIPT_MAX_AGE_MS - elapsed),
  );
  if (delay <= 0) {
    finishUnreadReceiptFailure(deps, parsed, error);
    return;
  }
  const token = retryState.token;
  const timer = setTimeout(() => {
    if (getRetryStates(deps.state).get(parsed.messageKey)?.token !== token) return;
    timers.delete(parsed.messageKey);
    submit(deps, parsed);
  }, delay);
  timers.set(parsed.messageKey, timer);
}
