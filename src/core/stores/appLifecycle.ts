import { defineStore } from "pinia";
import { readStartupPermissions } from "./appLifecyclePermissions";
import { createAppLifecycleRuntime } from "./appLifecycleRuntime";
import { createAppLifecycleState } from "./appLifecycleState";
import { createAppLifecycleStores } from "./appLifecycleStores";

export type AppState =
  | "PERMISSIONS"
  | "BOOTING"
  | "CONNECTING"
  | "PRELOADING"
  | "READY"
  | "ERROR";

export interface CoreStatus {
  status: "initializing" | "ready" | "error" | "none";
  message: string;
}

export interface PermissionStatus {
  notification: boolean;
  ring: boolean;
  storage: boolean;
  battery: boolean;
  // 这些字段描述可选能力或诊断状态，不属于下方的启动硬门禁。
  backgroundRestricted: boolean;
  requiresManualPowerManagement: boolean;
  microphone: boolean;
  camera: boolean;
  overlay: boolean;
  location: boolean;
}

export function hasRequiredStartupPermissions(
  permissions:
    | Pick<PermissionStatus, "notification" | "ring" | "storage" | "battery">
    | null
    | undefined,
  listener: { enabled?: unknown } | null | undefined,
): boolean {
  return (
    permissions?.notification === true &&
    permissions?.ring === true &&
    permissions?.storage === true &&
    permissions?.battery === true &&
    listener?.enabled === true
  );
}

async function checkRequiredPermissions() {
  const { permissions, listener } = await readStartupPermissions();
  const granted = hasRequiredStartupPermissions(permissions, listener);
  if (!granted) {
    console.log("[Lifecycle] Missing permissions, waiting for user action", {
      ...permissions,
      listener: listener?.enabled === true,
    });
  }
  return granted;
}

export const useAppLifecycleStore = defineStore("appLifecycle", () => {
  const lifecycle = createAppLifecycleState();
  const stores = createAppLifecycleStores();
  const runtime = createAppLifecycleRuntime(
    lifecycle,
    stores,
    checkRequiredPermissions,
  );
  return { ...lifecycle, ...runtime };
});
