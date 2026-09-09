import { onMounted, onUnmounted, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  addPluginListener,
  type PluginListener,
} from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useAppLifecycleStore } from "../stores/appLifecycle";
import { useChatStreamStore } from "../stores/chatStreamStore";
import { ForegroundStateController } from "./useAppLifecycleForeground";
import {
  createRecoveryController,
  type RecoveryController,
  type RecoveryReason,
} from "./useAppLifecycleRecoveryController";

type LifecycleStore = ReturnType<typeof useAppLifecycleStore>;
type StreamStore = ReturnType<typeof useChatStreamStore>;

interface NativeLifecyclePayload {
  state?: string;
}

interface RuntimeBindings {
  unlisten: UnlistenFn | null;
  nativeLifecycleListener: PluginListener | null;
}

function isAssistantLifecycleWindow() {
  if (typeof window === "undefined") return false;
  const { hash, pathname, search } = window.location;
  const hashPath = hash.slice(1).split("?")[0];
  if (
    search.includes("mode=floating") ||
    hash.includes("mode=floating") ||
    pathname.endsWith("/floating.html") ||
    hashPath === "/assistant"
  ) {
    return true;
  }
  try {
    return getCurrentWebviewWindow().label === "assistant";
  } catch {
    return false;
  }
}

function createVisibilityHandlers(
  lifecycleStore: LifecycleStore,
  requestRecovery: (reason: RecoveryReason) => Promise<void>,
  foregroundState: ForegroundStateController,
) {
  const handleVisibilityChange = () => {
    if (typeof document === "undefined") return;
    const wasBackground = lifecycleStore.isBackground;
    lifecycleStore.isBackground = document.hidden;
    if (wasBackground !== document.hidden) {
      void foregroundState.sync(!document.hidden);
    }
    console.log(`[useAppLifecycle] 可见性变化：hidden=${document.hidden}`);
  };
  const handleOnline = () => {
    console.log("[useAppLifecycle] 设备恢复联网，触发流恢复。");
    void requestRecovery("network-online");
  };
  return { handleVisibilityChange, handleOnline };
}

function installStateWatchers(
  lifecycleStore: LifecycleStore,
  requestRecovery: (reason: RecoveryReason) => Promise<void>,
  isRecoveryPending: () => boolean,
) {
  const stopReadyWatch = watch(
    () => lifecycleStore.state,
    (state) => {
      if (state === "READY" && isRecoveryPending()) {
        void requestRecovery("foreground");
      }
    },
  );
  const stopBackgroundWatch = watch(
    () => lifecycleStore.isBackground,
    (isBackground, wasBackground) => {
      if (isBackground) {
        document.documentElement.classList.add("vcp-paused-animations");
        console.log("[useAppLifecycle] 应用进入后台，暂停动画。");
        return;
      }
      document.documentElement.classList.remove("vcp-paused-animations");
      console.log("[useAppLifecycle] 应用回到前台，恢复动画。");
      if (wasBackground) void requestRecovery("foreground");
    },
    { immediate: true },
  );
  return { stopReadyWatch, stopBackgroundWatch };
}

async function registerTauriLifecycleListener(
  bindings: RuntimeBindings,
  lifecycleStore: LifecycleStore,
  requestRecovery: (reason: RecoveryReason) => Promise<void>,
  isDisposed: () => boolean,
) {
  try {
    const stopListening = await listen<{ state: string }>(
      "vcp-lifecycle-changed",
      (event) =>
        handleTauriLifecycleEvent(
          event.payload.state,
          lifecycleStore,
          requestRecovery,
        ),
    );
    if (isDisposed()) stopListening();
    else bindings.unlisten = stopListening;
  } catch (error) {
    console.error("[useAppLifecycle] 注册 Tauri 前后台监听失败：", error);
  }
}

function handleTauriLifecycleEvent(
  state: string,
  lifecycleStore: LifecycleStore,
  requestRecovery: (reason: RecoveryReason) => Promise<void>,
) {
  console.log(`[useAppLifecycle] 收到前后台事件：state=${state}`);
  if (state === "pause" || state === "stop") {
    lifecycleStore.isBackground = true;
  } else if (state === "resume") {
    lifecycleStore.isBackground = false;
    void requestRecovery("lifecycle-resume");
  }
}

async function registerNativeLifecycleListener(
  bindings: RuntimeBindings,
  lifecycleStore: LifecycleStore,
  requestRecovery: (reason: RecoveryReason) => Promise<void>,
  foregroundState: ForegroundStateController,
  isDisposed: () => boolean,
) {
  try {
    const listener = await addPluginListener<NativeLifecyclePayload>(
      "vcp-mobile",
      "lifecycle",
      (payload) =>
        handleNativeLifecycleEvent(
          payload,
          lifecycleStore,
          requestRecovery,
          foregroundState,
        ),
    );
    if (isDisposed()) await listener.unregister();
    else bindings.nativeLifecycleListener = listener;
  } catch (error) {
    console.error("[useAppLifecycle] 注册原生前后台监听失败：", error);
  }
}

function handleNativeLifecycleEvent(
  payload: NativeLifecyclePayload,
  lifecycleStore: LifecycleStore,
  requestRecovery: (reason: RecoveryReason) => Promise<void>,
  foregroundState: ForegroundStateController,
) {
  const state = payload?.state;
  if (state !== "pause" && state !== "stop" && state !== "resume") return;
  const isForeground = state === "resume";
  const wasBackground = lifecycleStore.isBackground;
  lifecycleStore.isBackground = !isForeground;
  if (wasBackground !== !isForeground) {
    void foregroundState.sync(isForeground);
  }
  if (isForeground) void requestRecovery("lifecycle-resume");
}

function cleanupRuntime(
  bindings: RuntimeBindings,
  watchers: ReturnType<typeof installStateWatchers>,
  visibility: ReturnType<typeof createVisibilityHandlers>,
  recovery: RecoveryController,
  foregroundState: ForegroundStateController,
) {
  recovery.dispose();
  foregroundState.dispose();
  watchers.stopReadyWatch();
  watchers.stopBackgroundWatch();
  document.removeEventListener(
    "visibilitychange",
    visibility.handleVisibilityChange,
  );
  window.removeEventListener("online", visibility.handleOnline);
  bindings.unlisten?.();
  bindings.unlisten = null;
  const listener = bindings.nativeLifecycleListener;
  bindings.nativeLifecycleListener = null;
  if (listener) {
    void Promise.resolve()
      .then(() => listener.unregister())
      .catch((error) => {
        console.error("[useAppLifecycle] 注销原生前后台监听失败：", error);
      });
  }
}

export function installAppLifecycleRuntime(
  lifecycleStore: LifecycleStore,
  streamStore: StreamStore,
) {
  if (isAssistantLifecycleWindow()) {
    console.log("[useAppLifecycle] 当前为划词助手窗口，停用主窗口恢复逻辑。");
    return;
  }
  const recovery = createRecoveryController(lifecycleStore, streamStore);
  const foregroundState = new ForegroundStateController(lifecycleStore);
  const requestRecovery = recovery.request.bind(recovery);
  const isRecoveryDisposed = recovery.isDisposed.bind(recovery);
  const isRecoveryPending = recovery.isPending.bind(recovery);
  const visibility = createVisibilityHandlers(
    lifecycleStore,
    requestRecovery,
    foregroundState,
  );
  const watchers = installStateWatchers(
    lifecycleStore,
    requestRecovery,
    isRecoveryPending,
  );
  const bindings: RuntimeBindings = {
    unlisten: null,
    nativeLifecycleListener: null,
  };

  onMounted(async () => {
    lifecycleStore.isBackground = document.hidden;
    void foregroundState.sync(!document.hidden);
    document.addEventListener(
      "visibilitychange",
      visibility.handleVisibilityChange,
    );
    window.addEventListener("online", visibility.handleOnline);
    await registerTauriLifecycleListener(
      bindings,
      lifecycleStore,
      requestRecovery,
      isRecoveryDisposed,
    );
    await registerNativeLifecycleListener(
      bindings,
      lifecycleStore,
      requestRecovery,
      foregroundState,
      isRecoveryDisposed,
    );
    void requestRecovery("mount");
  });
  onUnmounted(() =>
    cleanupRuntime(bindings, watchers, visibility, recovery, foregroundState),
  );
}
