import { invoke } from "@tauri-apps/api/core";
import { parseCommandError } from "./contract";
import type { SyncStatus } from "./types";

export interface StopSyncOptions {
  force: boolean;
  shouldStop: boolean;
  isOpen: boolean;
  setStartAttempt: () => void;
  clearSession: () => void;
  setStatus: (status: SyncStatus) => void;
  setCanDismiss: (value: boolean) => void;
  releaseScreen: () => void;
  pushLog: (level: string, message: string) => void;
  setTerminalError: (error: ReturnType<typeof parseCommandError>) => void;
}

export const stopSyncForSwitch = async (options: StopSyncOptions) => {
  if (!options.force && !options.shouldStop) return;
  options.setStartAttempt();
  options.clearSession();
  if (options.isOpen) {
    options.setStatus("stopping");
    options.setCanDismiss(false);
  }
  options.releaseScreen();
  try {
    await invoke("stop_sync");
    if (options.isOpen) {
      options.setStatus("stopped");
      options.setCanDismiss(true);
      options.pushLog("info", "同步已停止，线路可以切换");
    }
  } catch (error: unknown) {
    if (options.isOpen) {
      const terminal = parseCommandError(error, "STOP_SYNC_FAILED");
      options.setTerminalError(terminal);
      options.pushLog("error", terminal.message);
    }
    throw new Error("同步停止失败");
  }
};
