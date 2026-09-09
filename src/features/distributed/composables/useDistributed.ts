import { onUnmounted, readonly, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type DistributedConnectionState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "disconnecting";

export interface DistributedStatus {
  state: DistributedConnectionState;
  connected: boolean;
  server_id: string | null;
  client_id: string | null;
  registered_tools: number;
  last_error: string | null;
  session_id: number;
}

const status = ref<DistributedStatus>(emptyStatus());
const loading = ref(false);
let listenerPromise: Promise<void> | null = null;
let unlisten: UnlistenFn | null = null;
let listenerCount = 0;
let statusRevision = 0;
let latestEventSession = 0;
let hasReceivedEvent = false;

function emptyStatus(): DistributedStatus {
  return {
    state: "disconnected",
    connected: false,
    server_id: null,
    client_id: null,
    registered_tools: 0,
    last_error: null,
    session_id: 0,
  };
}

function normalizeStatus(value: unknown): DistributedStatus | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Partial<DistributedStatus>;
  const sessionId = typeof raw.session_id === "number" ? raw.session_id : 0;
  if (!Number.isSafeInteger(sessionId) || sessionId < 0) return null;
  const states: DistributedConnectionState[] = [
    "disconnected",
    "connecting",
    "connected",
    "disconnecting",
  ];
  if (!states.includes(raw.state as DistributedConnectionState)) return null;
  return {
    state: raw.state as DistributedConnectionState,
    connected: raw.connected === true,
    server_id: typeof raw.server_id === "string" ? raw.server_id : null,
    client_id: typeof raw.client_id === "string" ? raw.client_id : null,
    registered_tools:
      typeof raw.registered_tools === "number" && raw.registered_tools >= 0
        ? raw.registered_tools
        : 0,
    last_error: typeof raw.last_error === "string" ? raw.last_error : null,
    session_id: sessionId,
  };
}

function acceptStatus(
  next: DistributedStatus,
  source: "event" | "snapshot",
  revisionAtStart = statusRevision,
): boolean {
  const current = status.value;
  if (next.session_id < current.session_id) return false;
  if (source === "event") {
    if (next.session_id < latestEventSession) return false;
    latestEventSession = next.session_id;
    hasReceivedEvent = true;
  } else {
    if (hasReceivedEvent && next.session_id <= latestEventSession) return false;
    if (
      revisionAtStart !== statusRevision &&
      next.session_id <= current.session_id
    ) {
      return false;
    }
  }
  status.value = next;
  statusRevision += 1;
  return true;
}

async function setupListener(): Promise<void> {
  if (unlisten) return;
  if (!listenerPromise) {
    const registration = listen<DistributedStatus>(
      "vcp-distributed-status",
      (event) => {
        const next = normalizeStatus(event.payload);
        if (!next) {
          console.warn("[useDistributed] 忽略格式无效的分布式状态事件");
          return;
        }
        if (acceptStatus(next, "event")) {
          console.log("[useDistributed] 收到分布式状态事件：", next);
        }
      },
    );
    const pending = registration
      .then((stopListening) => {
        if (listenerCount <= 0) {
          stopListening();
          return;
        }
        unlisten = stopListening;
      })
      .catch((error) => {
        console.error("[useDistributed] 注册分布式状态监听失败：", error);
        throw error;
      });
    listenerPromise = pending;
    void pending.then(
      () => {
        if (listenerPromise === pending) listenerPromise = null;
      },
      () => {
        if (listenerPromise === pending) listenerPromise = null;
      },
    );
  }
  await listenerPromise;
}

function teardownListener(): void {
  if (listenerCount > 0) return;

  // 事件水位只属于当前监听周期。完全脱离监听后，下一位消费者必须能用
  // 同一 session 的权威快照校正离线期间发生的状态变化。
  latestEventSession = 0;
  hasReceivedEvent = false;

  if (!unlisten) return;
  const stopListening = unlisten;
  unlisten = null;
  try {
    stopListening();
  } catch (error) {
    console.error("[useDistributed] 注销分布式状态监听失败：", error);
  }
}

export function useDistributed() {
  const active = ref(false);

  async function activate(): Promise<void> {
    if (active.value) return;
    active.value = true;
    listenerCount += 1;
    try {
      await setupListener();
      if (!active.value) return;
      await refreshStatus();
    } catch (error) {
      active.value = false;
      listenerCount = Math.max(0, listenerCount - 1);
      teardownListener();
      console.error(
        "[useDistributed] 激活分布式状态失败，已回滚监听所有权：",
        error,
      );
    }
  }

  function deactivate(): void {
    if (!active.value) return;
    active.value = false;
    listenerCount = Math.max(0, listenerCount - 1);
    teardownListener();
  }

  onUnmounted(deactivate);

  async function refreshStatus(): Promise<void> {
    const revisionAtStart = statusRevision;
    loading.value = true;
    try {
      const snapshot = normalizeStatus(
        await invoke<DistributedStatus>("get_distributed_status"),
      );
      if (snapshot) acceptStatus(snapshot, "snapshot", revisionAtStart);
    } catch (error) {
      console.warn("[useDistributed] 获取分布式状态失败：", error);
    } finally {
      loading.value = false;
    }
  }

  return {
    status: readonly(status),
    loading: readonly(loading),
    activate,
    deactivate,
    refreshStatus,
  };
}

export function updateDistributedState(
  state: DistributedConnectionState,
  sessionId = status.value.session_id,
): void {
  const next = {
    ...status.value,
    state,
    connected: state === "connected",
    session_id: sessionId,
  };
  const normalized = normalizeStatus(next);
  if (normalized) acceptStatus(normalized, "event");
}
