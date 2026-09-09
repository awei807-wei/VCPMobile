import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  acquireScreenKeep,
  releaseScreenKeep,
} from "../composables/useScreenKeeper";
import { useConnectionSwitchGuardStore } from "./connectionSwitchGuard";
import { normalizeConnectionProfileId, useSettingsStore } from "./settings";
import {
  MAX_BUFFERED_SESSION_EVENTS,
  parseCommandError,
} from "./syncSession/contract";
import {
  buildDiagnostics,
  copyTextToClipboard,
  sanitizeDiagnosticText,
} from "./syncSession/diagnostics";
import { createSyncEventHandler } from "./syncSession/events";
import { registerSyncListeners } from "./syncSession/listeners";
import { startSyncAttempt } from "./syncSession/start";
import { stopSyncForSwitch } from "./syncSession/stop";
import { closeSyncSession } from "./syncSession/close";
import {
  emptyProgress,
  emptySummary,
  type BufferedSessionEvent,
  type SessionEventKind,
  type SyncProgress,
  type SyncStatus,
  type SyncTerminalError,
} from "./syncSession/types";

const readSessionId = (payload: Record<string, unknown>) => {
  const value = payload.sessionId;
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? value
    : null;
};

const readAttemptId = (payload: Record<string, unknown>) => {
  const value = payload.attemptId;
  if (value === undefined) return 0;
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : null;
};

export const useSyncSessionStore = defineStore("syncSession", () => {
  const settingsStore = useSettingsStore();
  const switchGuardStore = useConnectionSwitchGuardStore();

  const isOpen = ref(false);
  const canDismiss = ref(true);
  const status = ref<SyncStatus>("idle");
  const activeSessionId = ref<number | null>(null);
  const activeAttemptId = ref(0);
  const summary = ref(emptySummary());
  const terminalError = ref<SyncTerminalError | null>(null);
  const retryInFlight = ref(false);
  const activeTab = ref<"live" | "history">("live");
  const needsReload = ref(false);
  const logs = ref<
    { id: string; level: string; message: string; time: string }[]
  >([]);
  const progressData = ref<SyncProgress>(emptyProgress());

  let unlistenFns: UnlistenFn[] = [];
  let listenerSetup: Promise<void> | null = null;
  let viewGeneration = 0;
  let startAttempt = 0;
  let awaitingSessionId = false;
  let bufferedSessionEvents: BufferedSessionEvent[] = [];
  let runProfileId: string | null = null;
  let screenKeepHeld = false;

  const isActive = computed(() =>
    ["connecting", "connected", "retrying", "stopping"].includes(status.value),
  );
  const isTerminal = () =>
    ["error", "completed", "completed_with_warnings", "stopped"].includes(
      status.value,
    );
  const currentProfileId = () =>
    normalizeConnectionProfileId(
      settingsStore.settings?.activeConnectionProfileId,
    );
  const isCurrentProfile = () =>
    runProfileId === null || runProfileId === currentProfileId();
  const isCurrentView = (generation: number) =>
    isOpen.value && generation === viewGeneration && isCurrentProfile();
  const isCurrentRun = (generation: number, attempt: number) =>
    isCurrentView(generation) && attempt === startAttempt;

  const retainScreen = () => {
    if (screenKeepHeld) return;
    screenKeepHeld = true;
    acquireScreenKeep();
  };

  const releaseScreen = () => {
    if (!screenKeepHeld) return;
    screenKeepHeld = false;
    releaseScreenKeep();
  };

  const pushLog = (level: string, message: string) => {
    const id = `${Date.now()}_${Math.random().toString(36).slice(2, 9)}`;
    logs.value.push({
      id,
      level,
      message: sanitizeDiagnosticText(message.trim()).slice(0, 400),
      time: new Date().toLocaleTimeString(),
    });
    if (logs.value.length > 200) logs.value.shift();
  };

  const eventHandler = createSyncEventHandler({
    status,
    canDismiss,
    activeSessionId,
    activeAttemptId,
    summary,
    terminalError,
    needsReload,
    logs,
    progressData,
    pushLog,
    releaseScreen,
  });

  const resetRunState = (preserveLogs: boolean) => {
    if (!preserveLogs) logs.value = [];
    summary.value = emptySummary();
    terminalError.value = null;
    progressData.value = emptyProgress();
    activeSessionId.value = null;
    activeAttemptId.value = 0;
    awaitingSessionId = false;
    bufferedSessionEvents = [];
    eventHandler.reset();
  };

  const setTerminalError = (error: SyncTerminalError) => {
    terminalError.value = error;
    status.value = "error";
    canDismiss.value = true;
    releaseScreen();
  };

  const open = () => {
    viewGeneration += 1;
    startAttempt += 1;
    cleanupListeners();
    isOpen.value = true;
    canDismiss.value = true;
    status.value = "idle";
    activeTab.value = "live";
    needsReload.value = false;
    retryInFlight.value = false;
    runProfileId = null;
    resetRunState(false);
    listenerSetup = registerSyncListeners(
      viewGeneration,
      isCurrentView,
      routeSessionEvent,
      (unlisten) => unlistenFns.push(unlisten),
    );
  };

  const routeSessionEvent = (kind: SessionEventKind, rawPayload: unknown) => {
    if (
      !rawPayload ||
      typeof rawPayload !== "object" ||
      Array.isArray(rawPayload)
    )
      return;
    const payload = rawPayload as Record<string, unknown>;
    if (!isCurrentProfile()) return;
    const sessionId = readSessionId(payload);
    if (sessionId === null) return;
    if (activeSessionId.value === sessionId) {
      const attemptId = readAttemptId(payload);
      if (attemptId !== null && eventHandler.acceptAttempt(attemptId)) {
        eventHandler.apply(kind, payload);
      }
      return;
    }
    if (activeSessionId.value === null && awaitingSessionId) {
      bufferedSessionEvents.push({ kind, payload });
      if (bufferedSessionEvents.length > MAX_BUFFERED_SESSION_EVENTS) {
        bufferedSessionEvents.shift();
      }
    }
  };

  const cleanupListeners = () => {
    unlistenFns.forEach((unlisten) => unlisten());
    unlistenFns = [];
  };

  const beginSync = (preserveLogs: boolean) => {
    if (status.value !== "idle") return Promise.resolve();
    const generation = viewGeneration;
    const attempt = ++startAttempt;
    return startSyncAttempt({
      preserveLogs,
      generation,
      attempt,
      listenerSetup,
      isCurrentRun,
      currentProfileId,
      setRunProfileId: (profileId) => {
        runProfileId = profileId;
      },
      resetRunState,
      setStatus: (nextStatus) => {
        status.value = nextStatus;
      },
      setCanDismiss: (value) => {
        canDismiss.value = value;
      },
      retainScreen,
      pushLog,
      setTerminalError,
      setAwaitingSessionId: (value) => {
        awaitingSessionId = value;
      },
      setBufferedSessionEvents: (events) => {
        bufferedSessionEvents = events;
      },
      getBufferedSessionEvents: () => bufferedSessionEvents,
      setActiveSessionId: (sessionId) => {
        activeSessionId.value = sessionId;
      },
      readSessionId,
      readAttemptId,
      eventHandler,
    });
  };

  const startSync = () => {
    if (switchGuardStore.switching || status.value !== "idle") {
      return Promise.resolve();
    }
    return beginSync(false);
  };

  const stopForProfileSwitch = async (force = false) => {
    return stopSyncForSwitch({
      force,
      shouldStop: isActive.value || activeSessionId.value !== null,
      isOpen: isOpen.value,
      setStartAttempt: () => {
        startAttempt += 1;
      },
      clearSession: () => {
        activeSessionId.value = null;
        activeAttemptId.value = 0;
        awaitingSessionId = false;
        bufferedSessionEvents = [];
      },
      setStatus: (nextStatus) => {
        status.value = nextStatus;
      },
      setCanDismiss: (value) => {
        canDismiss.value = value;
      },
      releaseScreen,
      pushLog,
      setTerminalError,
    });
  };

  const retrySync = async () => {
    if (retryInFlight.value || !isTerminal() || status.value === "stopped")
      return;
    if (
      status.value === "error" &&
      terminalError.value &&
      !["manual", "after_user_action"].includes(terminalError.value.retryAction)
    )
      return;
    retryInFlight.value = true;
    startAttempt += 1;
    activeSessionId.value = null;
    activeAttemptId.value = 0;
    awaitingSessionId = false;
    bufferedSessionEvents = [];
    status.value = "retrying";
    canDismiss.value = false;
    releaseScreen();
    try {
      await invoke("stop_sync");
      if (!isOpen.value) return;
      pushLog("info", "──────── 新同步尝试 ────────");
      status.value = "idle";
      resetRunState(true);
      await beginSync(true);
    } catch (error: unknown) {
      if (isOpen.value) {
        const terminal = parseCommandError(error, "STOP_SYNC_FAILED");
        pushLog("error", terminal.message);
        setTerminalError(terminal);
      }
    } finally {
      if (isOpen.value) retryInFlight.value = false;
    }
  };

  const close = async () => {
    return closeSyncSession({
      isOpen: isOpen.value,
      canDismiss: canDismiss.value,
      needsReload: needsReload.value,
      invalidate: () => {
        needsReload.value = false;
        viewGeneration += 1;
        startAttempt += 1;
        isOpen.value = false;
        activeSessionId.value = null;
        activeAttemptId.value = 0;
        awaitingSessionId = false;
        bufferedSessionEvents = [];
        retryInFlight.value = false;
      },
      cleanupListeners,
      releaseScreen,
      setListenerSetup: (setup) => {
        listenerSetup = setup;
      },
    });
  };

  const copyDiagnostics = async () => {
    const diagnostic = await buildDiagnostics(
      status.value,
      activeSessionId.value,
      summary.value,
      terminalError.value,
    );
    await copyTextToClipboard(
      diagnostic,
      "脱敏诊断信息已复制到剪贴板",
      "复制诊断失败，请稍后再试",
      pushLog,
    );
  };

  const copyLogs = async () => {
    const text = logs.value
      .map((entry) => `[${entry.time}] ${entry.message}`)
      .join("\n");
    await copyTextToClipboard(
      text,
      "会话日志已复制到剪贴板",
      "复制日志失败，请稍后再试",
      pushLog,
    );
  };

  const markReloaded = () => {
    needsReload.value = false;
  };

  const switchTab = (tab: "live" | "history") => {
    if (isActive.value) return;
    activeTab.value = tab;
  };

  return {
    isOpen,
    canDismiss,
    status,
    isActive,
    activeSessionId,
    activeAttemptId,
    summary,
    terminalError,
    retryInFlight,
    needsReload,
    logs,
    progressData,
    activeTab,
    open,
    close,
    startSync,
    stopForProfileSwitch,
    retrySync,
    copyDiagnostics,
    copyLogs,
    markReloaded,
    switchTab,
  };
});
