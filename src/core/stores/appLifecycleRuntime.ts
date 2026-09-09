import { computed, type ComputedRef } from "vue";
import type { AppLifecycleState } from "./appLifecycleState";
import type { AppLifecycleStores } from "./appLifecycleStores";
import { createBootstrapActions } from "./appLifecycleBootstrap";
import { createCoreReadinessActions } from "./appLifecycleCore";
import {
  createLifecycleTransitions,
  observeVcpStatus,
} from "./appLifecycleTransitions";
import { createPreloadActions } from "./appLifecyclePreload";

export interface AppLifecycleRuntime {
  coreStatus: ComputedRef<
    AppLifecycleStores["notificationStore"]["vcpCoreStatus"]
  >;
  bootstrap: (force?: boolean) => Promise<void> | null;
  hydrateSystemStatus: () => Promise<void>;
}

export function createAppLifecycleRuntime(
  lifecycle: AppLifecycleState,
  stores: AppLifecycleStores,
  checkRequiredPermissions: () => Promise<boolean>,
): AppLifecycleRuntime {
  const updatePhaseLabel = (label: string) => {
    lifecycle.currentPhaseLabel.value = label;
    console.log(`[Lifecycle] ${label}`);
  };
  const core = createCoreReadinessActions({
    stores,
    updatePhaseLabel,
  });
  const transitions = createLifecycleTransitions(
    lifecycle,
    stores,
    core.cleanup,
  );
  const preload = createPreloadActions({
    stores,
    getState: () => lifecycle.state.value,
    cleanupConnectionWaiters: core.cleanup,
    setState: transitions.setState,
    updatePhaseLabel: transitions.updatePhaseLabel,
    markReady: () => {
      lifecycle.hasBootstrapped.value = true;
      lifecycle.isBootstrapping.value = false;
      transitions.setState("READY", "应用就绪");
    },
    fail: transitions.fail,
  });
  const bootstrap = createBootstrapActions({
    state: lifecycle.state,
    errorMsg: lifecycle.errorMsg,
    isBootstrapping: lifecycle.isBootstrapping,
    hasBootstrapped: lifecycle.hasBootstrapped,
    setState: transitions.setState,
    checkRequiredPermissions,
    hydrateSystemStatus: core.hydrateSystemStatus,
    waitForCoreReady: core.waitForCoreReady,
    initTheme: () => stores.themeStore.initTheme(),
    startPreloading: preload.startPreloading,
    fail: transitions.fail,
  });
  observeVcpStatus(lifecycle, stores);
  return {
    coreStatus: computed(() => stores.notificationStore.vcpCoreStatus),
    bootstrap: bootstrap.bootstrap,
    hydrateSystemStatus: core.hydrateSystemStatus,
  };
}
