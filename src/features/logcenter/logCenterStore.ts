// Fork adaptation: read-only log viewer. No remote clear command and no log persistence.
import { defineStore } from 'pinia';
import { computed, onScopeDispose, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useSettingsStore } from '../../core/stores/settings';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import { useConnectionSwitchGuardStore } from '../../core/stores/connectionSwitchGuard';
import { RequestEpoch } from './requestEpoch';
import { LINE_LIMIT_DEFAULT, clampLineLimit, splitLogChunk, stripAnsi } from './logText';

interface LogFetchResult {
  content: string;
  offset: number;
  path: string;
  fileSize: number;
  needFullReload: boolean;
}

type ScopeSnapshot = readonly [string, string, string, string];
const POLL_MS = 3000;
const MAX_BACKOFF_MS = 30000;
const MAX_LINE_CHARS = 65536;
const MAX_BUFFER_CHARS = 1048576;
const OMITTED = '[前部日志已省略] ';
const KEYS = {
  limit: 'vcp_log_limit',
  reverse: 'vcp_log_reverse',
  autoScroll: 'vcp_log_autoscroll',
} as const;

function readPreference(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
function writePreference(key: string, value: string): void {
  try { localStorage.setItem(key, value); } catch { /* Preferences only. */ }
}
function clipLine(line: string): string {
  if (line.length <= MAX_LINE_CHARS) return line;
  return OMITTED + line.slice(-(MAX_LINE_CHARS - OMITTED.length));
}
function boundedLines(input: string[], limit: number): string[] {
  const result = input.slice(-limit).map(clipLine);
  let chars = result.reduce((sum, line) => sum + line.length, 0);
  let first = 0;
  while (chars > MAX_BUFFER_CHARS && first < result.length - 1) {
    chars -= result[first++].length;
  }
  return result.slice(first);
}
function validResult(value: unknown): value is LogFetchResult {
  if (!value || typeof value !== 'object') return false;
  const v = value as Record<string, unknown>;
  return typeof v.content === 'string' && typeof v.path === 'string'
    && typeof v.needFullReload === 'boolean'
    && typeof v.offset === 'number' && Number.isSafeInteger(v.offset) && v.offset >= 0
    && typeof v.fileSize === 'number' && Number.isSafeInteger(v.fileSize) && v.fileSize >= 0;
}

export const useLogCenterStore = defineStore('logCenter', () => {
  const settingsStore = useSettingsStore();
  const lifecycleStore = useAppLifecycleStore();
  const switchGuard = useConnectionSwitchGuardStore();
  const requests = new RequestEpoch();
  const lines = ref<string[]>([]);
  const pendingFragment = ref('');
  const offset = ref(0);
  const logPath = ref('');
  const fileSize = ref(0);
  const hasSnapshot = ref(false);
  const sessionActive = ref(false);
  const isPaused = ref(false);
  const isLoading = ref(false);
  const error = ref<string | null>(null);
  const consecutiveFailures = ref(0);
  const newLineCount = ref(0);
  const logVersion = ref(0);
  const filterText = ref('');
  const lineLimit = ref(clampLineLimit(Number(readPreference(KEYS.limit)) || LINE_LIMIT_DEFAULT));
  const isReverse = ref(readPreference(KEYS.reverse) === '1');
  const autoScroll = ref(readPreference(KEYS.autoScroll) !== '0');
  let timer: ReturnType<typeof setTimeout> | null = null;
  let forceFull = true;
  let needsRun = false;
  let microtaskQueued = false;
  let disposed = false;

  // Credentials are compared in memory only. Never log, persist, or send this tuple.
  function scopeSnapshot(): ScopeSnapshot {
    const settings = settingsStore.settings;
    return [
      settings?.activeConnectionProfileId || 'lan',
      settings?.vcpServerUrl || '',
      settings?.adminUsername || '',
      settings?.adminPassword || '',
    ];
  }
  function sameScope(snapshot: ScopeSnapshot): boolean {
    const now = scopeSnapshot();
    return snapshot.every((item, index) => item === now[index]);
  }
  const canRun = computed(() => sessionActive.value && !isPaused.value
    && !lifecycleStore.isBackground && !switchGuard.switching && !!settingsStore.settings);
  const requestBlocked = computed(() => !canRun.value);
  const isPolling = computed(() => canRun.value);
  const sourceLabel = computed(() => scopeSnapshot()[0] === 'wan' ? '外网' : '内网');
  const matchedLines = computed(() => {
    const keyword = filterText.value.trim().toLowerCase();
    return keyword ? lines.value.filter(line => line.toLowerCase().includes(keyword)) : lines.value;
  });
  const displayedLines = computed(() => isReverse.value ? [...matchedLines.value].reverse() : matchedLines.value);
  const totalBuffered = computed(() => lines.value.length);
  const matchedCount = computed(() => matchedLines.value.length);

  function clearTimer(): void {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  }
  function clearBuffer(): void {
    lines.value = [];
    pendingFragment.value = '';
    offset.value = 0;
    logPath.value = '';
    fileSize.value = 0;
    hasSnapshot.value = false;
    newLineCount.value = 0;
    logVersion.value += 1;
  }
  function invalidate(clear: boolean): void {
    requests.invalidate();
    clearTimer();
    needsRun = false;
    forceFull = true;
    isLoading.value = false;
    error.value = null;
    consecutiveFailures.value = 0;
    if (clear) clearBuffer();
    // Do not reset requests.inFlight: obsolete native invokes still exist.
  }
  function queuePump(): void {
    if (disposed) return;
    needsRun = true;
    clearTimer();
    if (microtaskQueued) return;
    microtaskQueued = true;
    queueMicrotask(() => {
      microtaskQueued = false;
      void pump();
    });
  }
  function scheduleNext(): void {
    if (disposed || !canRun.value || timer !== null || requests.currentInFlight > 0) return;
    const delay = Math.min(MAX_BACKOFF_MS, POLL_MS * 2 ** Math.min(consecutiveFailures.value, 4));
    timer = setTimeout(() => { timer = null; queuePump(); }, delay);
  }
  function applyResult(data: LogFetchResult, incremental: boolean): void {
    if (incremental && !data.content) {
      offset.value = data.offset;
      logPath.value = data.path;
      fileSize.value = data.fileSize;
      return;
    }
    const carry = incremental ? pendingFragment.value : '';
    const chunk = splitLogChunk(stripAnsi(data.content), carry);
    const base = incremental
      ? (pendingFragment.value && lines.value.length ? lines.value.slice(0, -1) : lines.value)
      : [];
    lines.value = boundedLines([...base, ...chunk.lines], lineLimit.value);
    pendingFragment.value = clipLine(chunk.trailing);
    offset.value = data.offset;
    logPath.value = data.path;
    fileSize.value = data.fileSize;
    hasSnapshot.value = true;
    forceFull = false;
    newLineCount.value = incremental ? newLineCount.value + chunk.lines.length : 0;
    logVersion.value += 1;
  }
  async function pump(): Promise<void> {
    if (disposed || !canRun.value || !needsRun) return;
    const ticket = requests.begin();
    if (!ticket) return; // A finishing request will wake the newest pending attempt.
    needsRun = false;
    isLoading.value = true;
    const snapshot = scopeSnapshot();
    const incremental = hasSnapshot.value && !forceFull;
    const requestOffset = incremental ? offset.value : 0;
    try {
      const data: unknown = await invoke('logcenter_fetch', {
        incremental,
        offset: requestOffset,
        expectedProfileId: snapshot[0],
        expectedServerUrl: snapshot[1],
      });
      if (disposed || !requests.isCurrent(ticket) || !canRun.value || !sameScope(snapshot)) return;
      if (!validResult(data)) throw new Error('服务器日志响应不符合约定');
      if (data.needFullReload) {
        clearBuffer();
        forceFull = true;
        if (incremental) {
          needsRun = true; // Exactly one immediate full retry after rotation.
        } else {
          throw new Error('服务器全量响应仍要求重载，请稍后重试');
        }
      } else {
        if (incremental && data.offset < requestOffset) {
          forceFull = true;
          throw new Error('服务器日志游标回退，下次将全量重载');
        }
        applyResult(data, incremental);
      }
      error.value = null;
      consecutiveFailures.value = 0;
    } catch (cause: unknown) {
      if (!disposed && requests.isCurrent(ticket) && canRun.value && sameScope(snapshot)) {
        error.value = (cause instanceof Error ? cause.message : String(cause)).slice(0, 300);
        consecutiveFailures.value += 1;
      }
    } finally {
      requests.finish(ticket);
      if (!disposed) isLoading.value = requests.currentInFlight > 0;
      if (!disposed && canRun.value) {
        if (needsRun) queuePump();
        else scheduleNext();
      }
    }
  }

  function startSession(): void { sessionActive.value = true; }
  function stopSession(): void {
    sessionActive.value = false;
    invalidate(false);
  }
  function resetSession(): void {
    sessionActive.value = false;
    invalidate(true);
  }
  function refresh(): void {
    if (!canRun.value || disposed) return;
    invalidate(false);
    queuePump();
  }
  function togglePause(): void { isPaused.value = !isPaused.value; }
  function setLineLimit(raw: number): void {
    lineLimit.value = clampLineLimit(raw);
    lines.value = boundedLines(lines.value, lineLimit.value);
    writePreference(KEYS.limit, String(lineLimit.value));
    logVersion.value += 1;
  }
  function toggleReverse(): void {
    isReverse.value = !isReverse.value;
    writePreference(KEYS.reverse, isReverse.value ? '1' : '0');
  }
  function toggleAutoScroll(): void {
    autoScroll.value = !autoScroll.value;
    writePreference(KEYS.autoScroll, autoScroll.value ? '1' : '0');
  }
  function acknowledgeNewLines(): void { newLineCount.value = 0; }
  function pollOnce(): void { if (canRun.value) queuePump(); }

  watch(scopeSnapshot, (next, previous) => {
    if (next.every((value, index) => value === previous[index])) return;
    invalidate(true);
    if (canRun.value) queuePump();
  }, { flush: 'sync' });
  // Invalidate on entering AND leaving visibility/pause/switch boundaries.
  watch(canRun, (enabled) => {
    invalidate(false);
    if (enabled) queuePump();
  }, { flush: 'sync' });
  // Clearing on switch begin also covers a failed switch back to the same profile.
  watch(() => switchGuard.switching, (switching) => {
    if (switching) invalidate(true);
    else if (canRun.value) queuePump();
  }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; invalidate(true); });

  return {
    lines, pendingFragment, offset, logPath, fileSize, isPaused, isLoading, error,
    lineLimit, isReverse, autoScroll, filterText, newLineCount, logVersion,
    displayedLines, totalBuffered, matchedCount, isPolling, requestBlocked, sourceLabel,
    startSession, stopSession, resetSession, refresh, togglePause, setLineLimit,
    toggleReverse, toggleAutoScroll, acknowledgeNewLines, pollOnce,
  };
});
