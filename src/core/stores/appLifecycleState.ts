import { computed, ref, type ComputedRef, type Ref } from "vue";
import type { AppState } from "./appLifecycle";

export interface AppLifecycleState {
  state: Ref<AppState>;
  errorMsg: Ref<string | null>;
  currentPhaseLabel: Ref<string>;
  isBootstrapping: Ref<boolean>;
  hasBootstrapped: Ref<boolean>;
  lastTransitionAt: Ref<number | null>;
  isBackground: Ref<boolean>;
  statusText: ComputedRef<string>;
}

export function createAppLifecycleState(): AppLifecycleState {
  const state = ref<AppState>("BOOTING");
  const errorMsg = ref<string | null>(null);
  const currentPhaseLabel = ref("准备启动...");
  const isBootstrapping = ref(false);
  const hasBootstrapped = ref(false);
  const lastTransitionAt = ref<number | null>(null);
  const isBackground = ref(false);
  const statusText = computed(() => {
    switch (state.value) {
      case "PERMISSIONS":
        return "正在检查系统权限...";
      case "BOOTING":
        return "正在初始化界面资源...";
      case "CONNECTING":
        return "正在连接核心服务...";
      case "PRELOADING":
        return currentPhaseLabel.value || "正在预加载核心数据...";
      case "ERROR":
        return errorMsg.value || "启动失败";
      case "READY":
      default:
        return "应用已就绪";
    }
  });
  return {
    state,
    errorMsg,
    currentPhaseLabel,
    isBootstrapping,
    hasBootstrapped,
    lastTransitionAt,
    isBackground,
    statusText,
  };
}
