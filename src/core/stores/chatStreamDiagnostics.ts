export function isStreamDebugEnabled(): boolean {
  return Boolean(import.meta.env.DEV && (window as any).__VCP_STREAM_DEBUG__);
}

export function recordStreamTrace(data: unknown): void {
  if (!isStreamDebugEnabled()) return;
  if (!(window as any).__VCP_STREAM_TRACES__) {
    (window as any).__VCP_STREAM_TRACES__ = [];
  }
  (window as any).__VCP_STREAM_TRACES__.push({
    timestamp: performance.now(),
    ...((data as object) || {}),
  });
}

export function streamDebugLog(...args: unknown[]): void {
  if (isStreamDebugEnabled()) console.warn(...args);
}
