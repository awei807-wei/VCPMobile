import { invoke } from "@tauri-apps/api/core";
import type { PermissionStatus } from "./appLifecycle";

export interface StartupPermissionSnapshot {
  permissions: PermissionStatus;
  listener: { enabled: boolean };
}

export async function readStartupPermissions(): Promise<StartupPermissionSnapshot> {
  const permissions = await invoke<PermissionStatus>(
    "plugin:vcp-mobile|check_all_permissions",
  );
  const listener = await invoke<{ enabled: boolean }>(
    "plugin:vcp-mobile|check_notification_listener_permission",
  );
  return { permissions, listener };
}
