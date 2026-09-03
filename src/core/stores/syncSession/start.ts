import { invoke } from "@tauri-apps/api/core";
import { localTerminalError, parseCommandError } from "./contract";
import type { BufferedSessionEvent } from "./types";

type BatteryStatus = { level: number; isPowerSaveMode: boolean };

interface EventHandler {
  acceptAttempt: (attemptId: number) => boolean;
  apply: (
    kind: BufferedSessionEvent["kind"],
    payload: Record<string, unknown>,
  ) => void;
}

export interface SyncStartOptions {
  preserveLogs: boolean;
  generation: number;
  attempt: number;
  listenerSetup: Promise<void> | null;
  isCurrentRun: (generation: number, attempt: number) => boolean;
  currentProfileId: () => string;
  setRunProfileId: (profileId: string) => void;
  resetRunState: (preserveLogs: boolean) => void;
  setStatus: (status: "connecting") => void;
  setCanDismiss: (value: boolean) => void;
  retainScreen: () => void;
  pushLog: (level: string, message: string) => void;
  setTerminalError: (error: ReturnType<typeof localTerminalError>) => void;
  setAwaitingSessionId: (value: boolean) => void;
  setBufferedSessionEvents: (events: BufferedSessionEvent[]) => void;
  getBufferedSessionEvents: () => BufferedSessionEvent[];
  setActiveSessionId: (sessionId: number) => void;
  readSessionId: (payload: Record<string, unknown>) => number | null;
  readAttemptId: (payload: Record<string, unknown>) => number | null;
  eventHandler: EventHandler;
}

const runBatteryPreflight = async (options: SyncStartOptions) => {
  const { generation, attempt, isCurrentRun, pushLog, setTerminalError } =
    options;
  try {
    const battery = await invoke<BatteryStatus>(
      "plugin:vcp-mobile|get_battery_status",
    );
    if (!isCurrentRun(generation, attempt)) return false;
    pushLog("success", "设备状态检查完成");
    if (battery?.isPowerSaveMode) {
      const error = localTerminalError("POWER_SAVE_MODE");
      pushLog("error", error.message);
      setTerminalError(error);
      return false;
    }
    if (battery?.level > 0 && battery.level < 30) {
      const error = localTerminalError("BATTERY_TOO_LOW");
      pushLog("error", error.message);
      setTerminalError(error);
      return false;
    }
  } catch {
    if (!isCurrentRun(generation, attempt)) return false;
    pushLog("warning", "无法确认设备状态，将继续同步");
  }
  return isCurrentRun(generation, attempt);
};

const runRemoteSessionStart = async (options: SyncStartOptions) => {
  const {
    generation,
    attempt,
    isCurrentRun,
    setAwaitingSessionId,
    setBufferedSessionEvents,
    getBufferedSessionEvents,
    setActiveSessionId,
    readSessionId,
    readAttemptId,
    eventHandler,
  } = options;
  if (!isCurrentRun(generation, attempt)) return;
  setAwaitingSessionId(true);
  setBufferedSessionEvents([]);
  const sessionId = await invoke<number>("start_manual_sync");
  if (!isCurrentRun(generation, attempt)) {
    await invoke("stop_sync").catch(() => undefined);
    return;
  }
  if (!Number.isSafeInteger(sessionId) || sessionId <= 0) {
    throw new Error("invalid session id");
  }
  setActiveSessionId(sessionId);
  setAwaitingSessionId(false);
  const pending = getBufferedSessionEvents();
  setBufferedSessionEvents([]);
  pending.forEach((event) => {
    if (readSessionId(event.payload) !== sessionId) return;
    const attemptId = readAttemptId(event.payload);
    if (attemptId !== null && eventHandler.acceptAttempt(attemptId)) {
      eventHandler.apply(event.kind, event.payload);
    }
  });
};

export const startSyncAttempt = async (options: SyncStartOptions) => {
  const {
    preserveLogs,
    generation,
    attempt,
    listenerSetup,
    isCurrentRun,
    currentProfileId,
    setRunProfileId,
    resetRunState,
    setStatus,
    setCanDismiss,
    retainScreen,
    pushLog,
    setTerminalError,
    setAwaitingSessionId,
    setBufferedSessionEvents,
  } = options;

  setRunProfileId(currentProfileId());
  resetRunState(preserveLogs);
  setStatus("connecting");
  setCanDismiss(false);
  retainScreen();
  try {
    await listenerSetup;
    if (!isCurrentRun(generation, attempt)) return;
    if (!(await runBatteryPreflight(options))) return;
    await runRemoteSessionStart(options);
  } catch (error: unknown) {
    setAwaitingSessionId(false);
    setBufferedSessionEvents([]);
    if (!isCurrentRun(generation, attempt)) return;
    const terminal = parseCommandError(error, "START_SYNC_FAILED");
    pushLog("error", terminal.message);
    setTerminalError(terminal);
  }
};
