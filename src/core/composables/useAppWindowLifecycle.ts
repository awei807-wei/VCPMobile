import { ref, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  getCurrentWebviewWindow,
  WebviewWindow,
} from "@tauri-apps/api/webviewWindow";
import type { useAppLifecycleStore } from "../stores/appLifecycle";
import type { useChatSessionStore } from "../stores/chatSessionStore";
import type { useNotificationStore } from "../stores/notification";
import type { useModalHistory } from "./useModalHistory";
import {
  reapplyScreenKeepIfActive,
  suspendPhysicalScreenKeep,
} from "./useScreenKeeper";

type LifecycleStore = ReturnType<typeof useAppLifecycleStore>;
type SessionStore = ReturnType<typeof useChatSessionStore>;
type NotificationStore = ReturnType<typeof useNotificationStore>;
type ModalHistory = ReturnType<typeof useModalHistory>;

export interface WindowLifecycleOptions {
  lifecycleStore: LifecycleStore;
  sessionStore: SessionStore;
  notificationStore: NotificationStore;
  modalHistory: ModalHistory;
}

interface ExitState {
  exitTimer: number | null;
  isWaitingExit: Ref<boolean>;
}

interface BackgroundState {
  isAppBackground: boolean;
}

const createExitState = (): ExitState => ({
  exitTimer: null,
  isWaitingExit: ref(false),
});

const clearExitTimer = (state: ExitState) => {
  state.isWaitingExit.value = false;
  if (state.exitTimer) clearTimeout(state.exitTimer);
  state.exitTimer = null;
};

const moveTaskToBackground = async () => {
  try {
    await invoke("plugin:vcp-mobile|move_task_to_back");
  } catch (error) {
    console.warn(
      "[Exit] Failed to move task to back, calling window close fallback:",
      error,
    );
    await getCurrentWebviewWindow().close();
  }
};

const handleRootExit = async (
  state: ExitState,
  notificationStore: NotificationStore,
) => {
  if (state.isWaitingExit.value) {
    clearExitTimer(state);
    await moveTaskToBackground();
    return;
  }

  state.isWaitingExit.value = true;
  notificationStore.addNotification({
    id: "vcp-exit-toast",
    title: "再按一次退出应用",
    message: "",
    type: "info",
    duration: 2000,
    toastOnly: true,
  });
  if (typeof navigator !== "undefined" && navigator.vibrate) {
    navigator.vibrate(50);
  }
  state.exitTimer = window.setTimeout(() => clearExitTimer(state), 2000);
};

const handleExitRequest = async (
  state: ExitState,
  options: WindowLifecycleOptions,
) => {
  const { lifecycleStore, sessionStore, modalHistory, notificationStore } =
    options;
  console.log(
    `[ExitRequest] KeyPressed! State: ${lifecycleStore.state}, Item: ${
      sessionStore.currentSelectedItem?.id || "NULL"
    }, Topic: ${sessionStore.currentTopicId}, Modals: ${modalHistory.modalStackLength()}`,
  );
  if (modalHistory.closeTopModal()) return;
  if (
    lifecycleStore.state === "READY" &&
    sessionStore.currentSelectedItem !== null
  ) {
    sessionStore.$patch((currentState) => {
      currentState.currentSelectedItem = null;
      currentState.currentTopicId = null;
    });
    return;
  }
  await handleRootExit(state, notificationStore);
};

const createBackgroundState = (): BackgroundState => ({
  isAppBackground: false,
});

const restoreForegroundState = (
  state: BackgroundState,
  lifecycleStore: LifecycleStore,
) => {
  if (!state.isAppBackground) return;
  state.isAppBackground = false;
  reapplyScreenKeepIfActive();
  invoke("set_vcp_log_heartbeat", { intervalMs: 15000 }).catch((error) =>
    console.error("[Lifecycle] Failed to restore foreground heartbeat:", error),
  );
  lifecycleStore
    .hydrateSystemStatus()
    .catch((error) =>
      console.error("[Lifecycle] Failed to hydrate system status:", error),
    );
};

const handleVcpLifecycle = (
  event: Event,
  isAssistant: Ref<boolean>,
  state: BackgroundState,
  lifecycleStore: LifecycleStore,
) => {
  if (isAssistant.value) return;
  const lifecycleState = (event as CustomEvent).detail?.state;
  if (lifecycleState === "stop" || lifecycleState === "pause") {
    if (state.isAppBackground) return;
    state.isAppBackground = true;
    suspendPhysicalScreenKeep();
    invoke("set_vcp_log_heartbeat", { intervalMs: 120000 }).catch((error) =>
      console.error("[Lifecycle] Failed to set background heartbeat:", error),
    );
    return;
  }
  if (lifecycleState === "resume") {
    restoreForegroundState(state, lifecycleStore);
  }
};

const createAssistantWindow = async () => {
  const existingWindow = await WebviewWindow.getByLabel("assistant");
  if (existingWindow) {
    await existingWindow.show();
    await existingWindow.setFocus();
    return;
  }
  const newWindow = new WebviewWindow("assistant", {
    url: "/#/assistant",
    title: "VCP 划词助手",
    transparent: true,
    decorations: false,
    visible: true,
  });
  newWindow.once("tauri://created", () =>
    console.log("[App] Assistant window created successfully!"),
  );
  newWindow.once("tauri://error", (event) =>
    console.error("[App] Failed to create assistant window:", event),
  );
};

const handleFloatingBallClick = async () => {
  console.log("[App] Floating ball clicked. Resolving assistant window...");
  try {
    await createAssistantWindow();
  } catch (error) {
    console.error("[App] Failed to resolve assistant window:", error);
  }
};

const detectAssistantWindow = (isAssistant: Ref<boolean>) => {
  isAssistant.value = window.location.search.includes("mode=floating");
  if (isAssistant.value) return;
  try {
    isAssistant.value = getCurrentWebviewWindow().label === "assistant";
  } catch {
    // 非 Tauri 环境没有窗口标签，保留默认主窗口状态。
  }
};

export function useAppWindowLifecycle(options: WindowLifecycleOptions) {
  const isAssistant = ref(false);
  const exitState = createExitState();
  const backgroundState = createBackgroundState();
  return {
    isAssistant,
    detectAssistantWindow: () => detectAssistantWindow(isAssistant),
    handleExitRequest: (event: Event) => {
      void event;
      return handleExitRequest(exitState, options);
    },
    handleVisibilityChange: () => {
      document.documentElement.classList.toggle(
        "vcp-paused-animations",
        document.hidden,
      );
    },
    handleVcpLifecycle: (event: Event) =>
      handleVcpLifecycle(
        event,
        isAssistant,
        backgroundState,
        options.lifecycleStore,
      ),
    handleFloatingBallClick,
  };
}
