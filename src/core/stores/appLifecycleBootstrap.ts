import type { Ref } from "vue";
import type { AppState } from "./appLifecycle";

interface BootstrapContext {
  state: Ref<AppState>;
  errorMsg: Ref<string | null>;
  isBootstrapping: Ref<boolean>;
  hasBootstrapped: Ref<boolean>;
  setState: (nextState: AppState, reason: string) => void;
  checkRequiredPermissions: () => Promise<boolean>;
  hydrateSystemStatus: () => Promise<void>;
  waitForCoreReady: () => Promise<void>;
  initTheme: () => Promise<void>;
  startPreloading: () => Promise<void>;
  fail: (message: string) => void;
}

export interface BootstrapActions {
  bootstrap: (force?: boolean) => Promise<void> | null;
}

export function createBootstrapActions(
  context: BootstrapContext,
): BootstrapActions {
  let bootstrapPromise: Promise<void> | null = null;

  const runBootstrap = async () => {
    try {
      context.isBootstrapping.value = true;
      context.errorMsg.value = null;
      context.hasBootstrapped.value = false;
      context.setState("PERMISSIONS", "检查系统权限完整性");
      if (!(await context.checkRequiredPermissions())) {
        bootstrapPromise = null;
        context.isBootstrapping.value = false;
        return;
      }
      context.setState("BOOTING", "开始前端主线程启动编排");
      await context.hydrateSystemStatus();
      context.setState("CONNECTING", "等待后端核心服务就绪");
      await Promise.all([context.initTheme(), context.waitForCoreReady()]);
      console.log("[Lifecycle] Theme init + core ready complete");
      await context.startPreloading();
      bootstrapPromise = null;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      bootstrapPromise = null;
      context.fail(message);
      throw error;
    }
  };

  const bootstrap = (force = false) => {
    if (context.isBootstrapping.value && force) {
      console.log(
        "[Lifecycle] Reusing existing bootstrap promise (force-in-progress ignored)",
      );
      return bootstrapPromise;
    }
    if (bootstrapPromise && !force) {
      console.log("[Lifecycle] Reusing existing bootstrap promise");
      return bootstrapPromise;
    }
    bootstrapPromise = runBootstrap();
    return bootstrapPromise;
  };

  return { bootstrap };
}
