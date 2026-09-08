import { onScopeDispose, watch } from "vue";
import type { AppLifecycleState } from "./appLifecycleState";
import type { AppLifecycleStores } from "./appLifecycleStores";

export interface LifecycleTransitions {
  setState: (
    nextState: AppLifecycleState["state"]["value"],
    reason: string,
  ) => void;
  updatePhaseLabel: (label: string) => void;
  fail: (message: string) => void;
}

export function createLifecycleTransitions(
  lifecycle: AppLifecycleState,
  stores: AppLifecycleStores,
  cleanupConnectionWaiters: () => void,
): LifecycleTransitions {
  const setState = (
    nextState: AppLifecycleState["state"]["value"],
    reason: string,
  ) => {
    lifecycle.state.value = nextState;
    lifecycle.lastTransitionAt.value = Date.now();
    console.log(`[Lifecycle] -> ${nextState} | ${reason}`);
  };

  const updatePhaseLabel = (label: string) => {
    lifecycle.currentPhaseLabel.value = label;
    console.log(`[Lifecycle] ${label}`);
  };

  const fail = (message: string) => {
    cleanupConnectionWaiters();
    lifecycle.errorMsg.value = message;
    lifecycle.isBootstrapping.value = false;
    setState("ERROR", message);
    stores.notificationStore.updateCoreStatus({
      status: "error",
      message,
      source: "Core",
    });
    console.error("[Lifecycle] FATAL:", message);
  };

  return { setState, updatePhaseLabel, fail };
}

export function observeVcpStatus(
  lifecycle: AppLifecycleState,
  stores: AppLifecycleStores,
): void {
  const unwatchVcpStatus = watch(
    () => stores.notificationStore.vcpStatus.status,
    async (newStatus, oldStatus) => {
      if (
        newStatus === "connected" &&
        oldStatus !== "connected" &&
        lifecycle.state.value === "READY"
      ) {
        await stores.assistantStore.fetchAgents();
        await stores.assistantStore.fetchGroups();
      }
    },
  );
  onScopeDispose(unwatchVcpStatus);
}
