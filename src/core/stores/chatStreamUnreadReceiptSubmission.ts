import type {
  ParsedStreamEvent,
  StreamProcessorDeps,
  UnreadMessageRetryState,
  UnreadReceiptActiveGuard,
  UnreadReceiptSubmission,
  UnreadReceiptSubmissionResult,
} from "./chatStreamProcessorSupport";
import {
  clearRetryState,
  createRetryState,
  finishUnreadReceiptFailure,
  getFailedKeys,
  getRetryStates,
  isPermanentUnreadReceiptError,
  isReceiptSubmissionActive,
  isReceiptTombstoned,
  rememberReceipt,
  scheduleUnreadReceiptRetry,
  UNREAD_RECEIPT_MAX_AGE_MS,
  UNREAD_RECEIPT_MAX_ATTEMPTS,
} from "./chatStreamUnreadReceiptState";

interface PreparedUnreadReceipt {
  completed: Set<string>;
  inFlight: Set<string>;
  retryState: UnreadMessageRetryState;
  isActive: UnreadReceiptActiveGuard;
}

function normalizeReceiptResult(
  result: void | boolean | UnreadReceiptSubmissionResult,
): UnreadReceiptSubmissionResult {
  if (result && typeof result === "object") {
    return {
      success: result.success === true,
      permanent: result.permanent === true,
      cancelled: result.cancelled === true,
    };
  }
  return { success: result !== false };
}

function clearPendingRetry(deps: StreamProcessorDeps, messageKey: string): void {
  const retryTimers = deps.state.unreadMessageRetryTimers;
  const pendingRetry = retryTimers?.get(messageKey);
  if (pendingRetry === undefined) return;
  clearTimeout(pendingRetry);
  retryTimers?.delete(messageKey);
}

function prepareUnreadReceipt(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
): PreparedUnreadReceipt | null {
  rememberReceipt(deps, parsed);
  if (isReceiptTombstoned(deps.state, parsed)) return null;
  const completed =
    deps.state.unreadMessageKeys ||
    (deps.state.unreadMessageKeys = new Set<string>());
  const inFlight =
    deps.state.unreadMessageInFlightKeys ||
    (deps.state.unreadMessageInFlightKeys = new Set<string>());
  const failed = getFailedKeys(deps.state);
  clearPendingRetry(deps, parsed.messageKey);
  if (
    completed.has(parsed.messageKey) ||
    failed.has(parsed.messageKey) ||
    inFlight.has(parsed.messageKey)
  )
    return null;

  const retryState =
    getRetryStates(deps.state).get(parsed.messageKey) ||
    createRetryState(deps, parsed);
  const elapsed = Math.max(0, Date.now() - retryState.startedAt);
  if (
    retryState.attempt >= UNREAD_RECEIPT_MAX_ATTEMPTS ||
    elapsed >= UNREAD_RECEIPT_MAX_AGE_MS
  ) {
    finishUnreadReceiptFailure(deps, parsed, "未读收据重试预算已耗尽");
    return null;
  }
  retryState.attempt += 1;
  inFlight.add(parsed.messageKey);
  return {
    completed,
    inFlight,
    retryState,
    isActive: () => isReceiptSubmissionActive(deps, parsed, retryState.token),
  };
}

function invokeUnreadReceipt(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  isActive: UnreadReceiptActiveGuard,
): UnreadReceiptSubmission {
  return deps.incrementTopicUnreadCountWithGuard
    ? deps.incrementTopicUnreadCountWithGuard(
        parsed.identity,
        parsed.messageId,
        isActive,
      )
    : deps.incrementTopicUnreadCount(parsed.identity, parsed.messageId);
}

function completeUnreadReceipt(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  prepared: PreparedUnreadReceipt,
  result: UnreadReceiptSubmissionResult,
): void {
  prepared.inFlight.delete(parsed.messageKey);
  const currentState = getRetryStates(deps.state).get(parsed.messageKey);
  if (!currentState || currentState.token !== prepared.retryState.token) return;
  if (result.success) {
    prepared.completed.add(parsed.messageKey);
    clearRetryState(deps, parsed.messageKey);
  } else if (result.cancelled) {
    clearRetryState(deps, parsed.messageKey);
  } else if (result.permanent) {
    finishUnreadReceiptFailure(deps, parsed, "永久性未读收据错误");
  } else {
    // A terminal event can arrive while topicListCounters is reconciling
    // a failed IPC call. Keep one delayed retry independent of future
    // stream events so that the receipt is not lost at stream end.
    scheduleUnreadReceiptRetry(
      deps,
      parsed,
      submitUnreadReceipt,
      "未读收据提交失败",
    );
  }
}

function handleUnreadReceiptError(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  prepared: PreparedUnreadReceipt,
  error: unknown,
): void {
  prepared.inFlight.delete(parsed.messageKey);
  console.error("[ChatStreamStore] 提交消息未读收据失败：", error);
  if (isPermanentUnreadReceiptError(error)) {
    finishUnreadReceiptFailure(deps, parsed, error);
  } else {
    scheduleUnreadReceiptRetry(deps, parsed, submitUnreadReceipt, error);
  }
}

function observeUnreadReceipt(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
  prepared: PreparedUnreadReceipt,
  submission: PromiseLike<void | boolean | UnreadReceiptSubmissionResult>,
): void {
  void Promise.resolve(submission)
    .then((value) =>
      completeUnreadReceipt(deps, parsed, prepared, normalizeReceiptResult(value)),
    )
    .catch((error) => {
      if (!prepared.isActive()) {
        completeUnreadReceipt(deps, parsed, prepared, {
          success: false,
          cancelled: true,
        });
        return;
      }
      const permanent = isPermanentUnreadReceiptError(error);
      if (permanent)
        console.error("[ChatStreamStore] 未读收据目标已不存在：", error);
      handleUnreadReceiptError(deps, parsed, prepared, error);
    });
}

export function submitUnreadReceipt(
  deps: StreamProcessorDeps,
  parsed: ParsedStreamEvent,
): void {
  const prepared = prepareUnreadReceipt(deps, parsed);
  if (!prepared) return;
  let submission: UnreadReceiptSubmission;
  try {
    submission = invokeUnreadReceipt(deps, parsed, prepared.isActive);
  } catch (error) {
    handleUnreadReceiptError(deps, parsed, prepared, error);
    return;
  }
  if (
    !submission ||
    typeof (submission as PromiseLike<unknown>).then !== "function"
  ) {
    completeUnreadReceipt(
      deps,
      parsed,
      prepared,
      normalizeReceiptResult(
        submission as void | boolean | UnreadReceiptSubmissionResult,
      ),
    );
    return;
  }
  observeUnreadReceipt(
    deps,
    parsed,
    prepared,
    submission as PromiseLike<
      void | boolean | UnreadReceiptSubmissionResult
    >,
  );
}
