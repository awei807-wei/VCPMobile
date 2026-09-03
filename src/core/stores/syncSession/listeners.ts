import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SessionEventKind } from "./types";

export const registerSyncListeners = async (
  generation: number,
  isCurrentView: (generation: number) => boolean,
  routeSessionEvent: (kind: SessionEventKind, payload: unknown) => void,
  addUnlistener: (unlisten: UnlistenFn) => void,
) => {
  const register = async (eventName: string, kind: SessionEventKind) => {
    const unlisten = await listen<unknown>(eventName, (event) => {
      if (isCurrentView(generation)) routeSessionEvent(kind, event.payload);
    });
    if (!isCurrentView(generation)) {
      unlisten();
      return;
    }
    addUnlistener(unlisten);
  };

  await Promise.all([
    register("vcp-log", "log"),
    register("vcp-sync-progress", "progress"),
    register("vcp-sync-status", "status"),
    register("vcp-sync-completed", "completed"),
  ]);
};
