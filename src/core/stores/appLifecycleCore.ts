import { invoke } from "@tauri-apps/api/core";
import { watch } from "vue";
import { updateDistributedState } from "../../features/distributed/composables/useDistributed";
import type { AppLifecycleStores } from "./appLifecycleStores";

const CONNECT_TIMEOUT_MS = 15000;

export interface CoreReadinessContext {
  stores: AppLifecycleStores;
  updatePhaseLabel: (label: string) => void;
}

export interface CoreReadinessActions {
  waitForCoreReady: () => Promise<void>;
  hydrateSystemStatus: () => Promise<void>;
  cleanup: () => void;
}

interface CoreWaiterState {
  timeoutId: ReturnType<typeof setTimeout> | null;
  stopWatch: (() => void) | null;
  cleanupPending: boolean;
  active: boolean;
}

function cleanupCoreWaiter(state: CoreWaiterState): void {
  if (!state.active) return;
  state.active = false;
  if (state.timeoutId) {
    clearTimeout(state.timeoutId);
    state.timeoutId = null;
  }
  if (state.stopWatch) {
    state.stopWatch();
    state.stopWatch = null;
    state.cleanupPending = false;
  } else {
    state.cleanupPending = true;
  }
}

function waitForCoreStatusChange(
  stores: AppLifecycleStores,
  waiter: CoreWaiterState,
): Promise<void> {
  waiter.active = true;
  return new Promise<void>((resolve, reject) => {
    let settled = false;
    const finish = (callback: () => void) => {
      if (settled) return;
      settled = true;
      cleanupCoreWaiter(waiter);
      callback();
    };
    waiter.timeoutId = setTimeout(
      () =>
        finish(() =>
          reject(new Error(`等待核心引擎就绪超时（${CONNECT_TIMEOUT_MS}ms）`)),
        ),
      CONNECT_TIMEOUT_MS,
    );
    waiter.stopWatch = watch(
      () => stores.notificationStore.vcpCoreStatus.status,
      (newStatus) => {
        if (newStatus === "ready") finish(resolve);
        if (newStatus === "error") {
          finish(() =>
            reject(
              new Error(
                stores.notificationStore.vcpCoreStatus.message ||
                  "核心引擎启动失败",
              ),
            ),
          );
        }
      },
      { immediate: true },
    );
    if (waiter.cleanupPending) {
      waiter.stopWatch?.();
      waiter.stopWatch = null;
      waiter.cleanupPending = false;
    }
  });
}

function createCoreWaiters(stores: AppLifecycleStores) {
  const waiter: CoreWaiterState = {
    timeoutId: null,
    stopWatch: null,
    cleanupPending: false,
    active: false,
  };
  return {
    cleanup: () => cleanupCoreWaiter(waiter),
    wait: () => waitForCoreStatusChange(stores, waiter),
  };
}

async function waitForCoreReady(
  context: CoreReadinessContext,
  waitForCoreStatusChange: () => Promise<void>,
): Promise<void> {
  context.updatePhaseLabel("检查核心服务状态...");
  const currentStatus = context.stores.notificationStore.vcpCoreStatus.status;
  console.log(
    `[Lifecycle] Checked core status from snapshot -> ${currentStatus}`,
  );
  if (currentStatus === "ready") return;
  if (currentStatus === "error") {
    const lastError = await invoke<string | null>("get_last_error");
    const message = lastError || "核心服务在初始化阶段发生崩溃";
    context.stores.notificationStore.updateCoreStatus({
      status: "error",
      message,
      source: "Core",
    });
    throw new Error(message);
  }
  context.updatePhaseLabel("等待核心就绪...");
  await waitForCoreStatusChange();
}

async function hydrateSystemStatus(
  context: CoreReadinessContext,
): Promise<void> {
  try {
    console.log("[Lifecycle] Fetching system status snapshot...");
    const snapshot = await invoke<{
      core: string;
      log: string;
      sync: string;
      distributed: string;
    }>("get_system_snapshot");
    context.stores.notificationStore.updateCoreStatus({
      status: snapshot.core as any,
      message:
        snapshot.core === "ready" ? "核心引擎已就绪" : "核心引擎初始化中...",
      source: "Core",
    });
    context.stores.notificationStore.updateStatus({
      status: snapshot.log as any,
      message: snapshot.log === "connected" ? "已连接" : "正在连接...",
      source: "VCPLog",
    });
    updateDistributedState(snapshot.distributed as any);
    console.log("[Lifecycle] Snapshot hydrated:", JSON.stringify(snapshot));
  } catch (error) {
    console.error("[Lifecycle] Failed to hydrate status snapshot:", error);
  }
}

export function createCoreReadinessActions(
  context: CoreReadinessContext,
): CoreReadinessActions {
  const waiters = createCoreWaiters(context.stores);
  return {
    waitForCoreReady: () => waitForCoreReady(context, waiters.wait),
    hydrateSystemStatus: () => hydrateSystemStatus(context),
    cleanup: waiters.cleanup,
  };
}
