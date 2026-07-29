import { onMounted, onUnmounted, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useAppLifecycleStore } from "../stores/appLifecycle";

interface ActiveGeneration {
  msgId: string;
  topicId: string;
  ownerId: string;
  ownerType: string;
  createdAt: number;
}

type RecoveryReason =
  | "mount"
  | "foreground"
  | "lifecycle-resume"
  | "network-online";

function isCoreNotReadyError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes("CORE_NOT_READY:");
}

/** 判断当前 WebView 是否为只承载划词助手的窗口。 */
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

export function useAppLifecycle() {
  const lifecycleStore = useAppLifecycleStore();
  const isAssistant = isAssistantLifecycleWindow();
  let unlisten: UnlistenFn | null = null;
  let disposed = false;
  let recoveryPending = false;
  let recoveryPromise: Promise<void> | null = null;

  /**
   * 获取并恢复中断的流式生成。
   * 每次 IPC 前都校验 READY，避免数据库状态尚未注册时调用后端命令。
   */
  const recoverInterruptedStreams = async () => {
    try {
      if (disposed || lifecycleStore.state !== "READY") {
        recoveryPending = !disposed;
        return;
      }

      const activeGenerations = await invoke<ActiveGeneration[]>(
        "get_active_generations",
      );
      if (!activeGenerations?.length) {
        console.log("[useAppLifecycle] No active generations to recover.");
        return;
      }

      console.log(
        `[useAppLifecycle] Found ${activeGenerations.length} active generations, attempting recovery...`,
      );

      for (const generation of activeGenerations) {
        if (disposed || lifecycleStore.state !== "READY") {
          recoveryPending = !disposed;
          console.log(
            "[useAppLifecycle] Core left READY, deferring remaining recovery.",
          );
          return;
        }

        try {
          console.log(
            `[useAppLifecycle] Recovering generation: msgId=${generation.msgId}, topicId=${generation.topicId}`,
          );
          await invoke("recover_active_generation", {
            msgId: generation.msgId,
          });
        } catch (err) {
          if (isCoreNotReadyError(err)) {
            console.info(
              `[useAppLifecycle] Core not ready while recovering ${generation.msgId}; skipped.`,
            );
            return;
          }

          console.warn(
            `[useAppLifecycle] Failed to recover generation ${generation.msgId}:`,
            err,
          );
        }
      }
    } catch (err) {
      if (isCoreNotReadyError(err)) {
        console.info(
          "[useAppLifecycle] Core not ready for stream recovery; skipped.",
        );
        return;
      }

      console.error("[useAppLifecycle] Failed to get active generations:", err);
    }
  };

  /**
   * 将挂载、回前台和网络恢复合并为单个恢复任务。
   * READY 前只记录一次待执行任务；已有任务运行时，后续触发复用同一 Promise。
   */
  const requestStreamRecovery = (reason: RecoveryReason) => {
    if (isAssistant || disposed) return Promise.resolve();

    if (recoveryPromise) {
      console.log(
        `[useAppLifecycle] Recovery already running; coalesced trigger: ${reason}`,
      );
      return recoveryPromise;
    }

    recoveryPending = true;
    if (lifecycleStore.state !== "READY") {
      console.log(
        `[useAppLifecycle] Core is ${lifecycleStore.state}; deferred recovery trigger: ${reason}`,
      );
      return Promise.resolve();
    }

    recoveryPending = false;
    recoveryPromise = recoverInterruptedStreams().finally(() => {
      recoveryPromise = null;

      if (recoveryPending && !disposed && lifecycleStore.state === "READY") {
        void requestStreamRecovery("foreground");
      }
    });
    return recoveryPromise;
  };

  if (isAssistant) {
    console.log(
      "[useAppLifecycle] Assistant window detected; main-window recovery disabled.",
    );
    return;
  }

  const handleVisibilityChange = () => {
    if (typeof document === "undefined") return;

    lifecycleStore.isBackground = document.hidden;
    console.log(
      `[useAppLifecycle] Visibility changed: hidden=${document.hidden}`,
    );
  };

  const handleOnline = () => {
    console.log("[useAppLifecycle] Device online. Triggering stream recovery.");
    void requestStreamRecovery("network-online");
  };

  const stopReadyWatch = watch(
    () => lifecycleStore.state,
    (state) => {
      if (state === "READY" && recoveryPending) {
        void requestStreamRecovery("foreground");
      }
    },
  );

  // 监听后台状态，控制全局动画挂起；仅真实的后台 -> 前台切换触发恢复。
  const stopBackgroundWatch = watch(
    () => lifecycleStore.isBackground,
    (isBackground, wasBackground) => {
      if (isBackground) {
        document.documentElement.classList.add("vcp-paused-animations");
        console.log(
          "[useAppLifecycle] App moved to background, pausing animations.",
        );
        return;
      }

      document.documentElement.classList.remove("vcp-paused-animations");
      console.log(
        "[useAppLifecycle] App moved to foreground, resuming animations.",
      );
      if (wasBackground) {
        void requestStreamRecovery("foreground");
      }
    },
    { immediate: true },
  );

  onMounted(async () => {
    document.addEventListener("visibilitychange", handleVisibilityChange);
    window.addEventListener("online", handleOnline);

    try {
      const stopListening = await listen<{ state: string }>(
        "vcp-lifecycle-changed",
        (event) => {
          const state = event.payload.state;
          console.log(
            `[useAppLifecycle] Received vcp-lifecycle-changed: state=${state}`,
          );

          if (state === "pause" || state === "stop") {
            lifecycleStore.isBackground = true;
          } else if (state === "resume") {
            lifecycleStore.isBackground = false;
            void requestStreamRecovery("lifecycle-resume");
          }
        },
      );

      if (disposed) {
        stopListening();
      } else {
        unlisten = stopListening;
      }
    } catch (err) {
      console.error(
        "[useAppLifecycle] Failed to setup Tauri lifecycle listener:",
        err,
      );
    }

    void requestStreamRecovery("mount");
  });

  onUnmounted(() => {
    disposed = true;
    recoveryPending = false;
    stopReadyWatch();
    stopBackgroundWatch();
    document.removeEventListener("visibilitychange", handleVisibilityChange);
    window.removeEventListener("online", handleOnline);
    unlisten?.();
    unlisten = null;
  });
}
