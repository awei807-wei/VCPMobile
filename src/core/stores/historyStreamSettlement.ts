export const HISTORY_STREAM_SETTLE_TIMEOUT_MS = 15_000;

export type HistoryStreamSettlementReason =
  | "final"
  | "empty"
  | "aborted"
  | "timeout";

export interface HistoryStreamSettlement {
  completion: Promise<HistoryStreamSettlementReason>;
  settle: (reason: HistoryStreamSettlementReason) => void;
  cleanup: () => void;
}

/**
 * 为 Channel 历史请求设置终止边界，并隔离迟到事件。
 * 原生命令可能先于终帧返回，也可能在传输失败后不再返回；两种路径都必须释放加载状态。
 */
export function createHistoryStreamSettlement(
  signal: AbortSignal,
  detachChannel: () => void,
  timeoutMs = HISTORY_STREAM_SETTLE_TIMEOUT_MS,
): HistoryStreamSettlement {
  let settled = false;
  let cleanedUp = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let resolveCompletion: (
    reason: HistoryStreamSettlementReason,
  ) => void = () => undefined;

  const completion = new Promise<HistoryStreamSettlementReason>((resolve) => {
    resolveCompletion = resolve;
  });

  const onAbort = () => {
    settle("aborted");
  };

  const cleanup = () => {
    if (cleanedUp) return;
    cleanedUp = true;
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    signal.removeEventListener("abort", onAbort);
    detachChannel();
  };

  const settle = (reason: HistoryStreamSettlementReason) => {
    if (settled) return;
    settled = true;
    cleanup();
    resolveCompletion(reason);
  };

  signal.addEventListener("abort", onAbort, { once: true });
  timer = setTimeout(() => settle("timeout"), timeoutMs);

  return { completion, settle, cleanup };
}
