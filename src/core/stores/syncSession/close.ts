import { invoke } from "@tauri-apps/api/core";
import { useDataReload } from "../../composables/useDataReload";
import { useNotificationStore } from "../notification";

export interface CloseSyncOptions {
  isOpen: boolean;
  canDismiss: boolean;
  needsReload: boolean;
  invalidate: () => void;
  cleanupListeners: () => void;
  releaseScreen: () => void;
  setListenerSetup: (setup: Promise<void> | null) => void;
}

export const closeSyncSession = async (options: CloseSyncOptions) => {
  if (!options.isOpen || !options.canDismiss) return;
  const shouldReload = options.needsReload;
  options.invalidate();
  options.cleanupListeners();
  options.setListenerSetup(null);
  options.releaseScreen();
  try {
    await invoke("stop_sync");
  } catch (error) {
    console.warn("[SyncSession] Failed to stop backend sync session", error);
  }
  if (!shouldReload) return;
  try {
    await useDataReload().performFullReload();
  } catch (error) {
    console.error("[SyncSession] Failed to reload synchronized data", error);
    useNotificationStore().addNotification({
      id: "sync_data_reload_failed",
      type: "error",
      title: "同步数据刷新失败",
      message: "界面未能重新加载同步后的数据，请重新打开应用。",
      toastOnly: false,
    });
  }
};
