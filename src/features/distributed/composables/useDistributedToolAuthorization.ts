import { invoke } from "@tauri-apps/api/core";
import { computed, readonly, ref } from "vue";
import { useNotificationStore } from "../../../core/stores/notification";
import { useDistributedAuthorization } from "./useDistributedAuthorization";
import type { PluginItem } from "./distributedToolTypes";
import type { DistributedToolState } from "./distributedToolState";

type ToolAuthorization = ReturnType<typeof useDistributedAuthorization>;
type NotificationStore = ReturnType<typeof useNotificationStore>;

let toolMutationTail: Promise<unknown> = Promise.resolve();
const pendingToolMutations = ref<Record<string, number>>({});
const pendingResetCount = ref(0);

export const distributedToolPendingIds = readonly(pendingToolMutations);
export const distributedToolPendingCount = computed(
  () => Object.keys(pendingToolMutations.value).length,
);
export const distributedResetPending = computed(
  () => pendingResetCount.value > 0,
);

function beginToolMutation(pluginId: string): void {
  pendingToolMutations.value = {
    ...pendingToolMutations.value,
    [pluginId]: (pendingToolMutations.value[pluginId] || 0) + 1,
  };
}

function endToolMutation(pluginId: string): void {
  const remaining = (pendingToolMutations.value[pluginId] || 1) - 1;
  const next = { ...pendingToolMutations.value };
  if (remaining > 0) next[pluginId] = remaining;
  else delete next[pluginId];
  pendingToolMutations.value = next;
}

export function isDistributedToolMutationPending(pluginId: string): boolean {
  return (pendingToolMutations.value[pluginId] || 0) > 0;
}

export function hasAnyDistributedToolMutationPending(): boolean {
  return distributedToolPendingCount.value > 0;
}

function enqueueToolMutation<T>(operation: () => Promise<T>): Promise<T> {
  const run = toolMutationTail.catch(() => undefined).then(operation);
  toolMutationTail = run.catch(() => undefined);
  return run;
}

async function readSensitivePermission(plugin: PluginItem): Promise<boolean> {
  const permissionType =
    plugin.id === "MobileLocation"
      ? "location"
      : plugin.id === "MobileNotification"
        ? "notification"
        : null;
  if (!permissionType) return true;
  const permissions = await invoke<Record<string, unknown>>(
    "plugin:vcp-mobile|check_all_permissions",
  );
  if (
    !permissions ||
    typeof permissions !== "object" ||
    !(permissionType in permissions)
  ) {
    throw new Error(`权限状态缺少 ${permissionType} 字段`);
  }
  if (permissions[permissionType] === true) return true;
  const notificationStore = useNotificationStore();
  notificationStore.addNotification({
    type: "info",
    title: "权限请求",
    message: `${plugin.name} 需要系统${permissionType === "location" ? "定位" : "通知"}权限，请在系统对话框中点击“允许”。`,
    toastOnly: true,
  });
  await invoke("plugin:vcp-mobile|request_android_permission", {
    pType: permissionType,
  });
  const retry = await invoke<Record<string, unknown>>(
    "plugin:vcp-mobile|check_all_permissions",
  );
  if (retry?.[permissionType] === true) return true;
  notificationStore.addNotification({
    type: "warning",
    title: "未获得权限",
    message: `由于未获得系统权限，开启 ${plugin.name} 失败。`,
    toastOnly: true,
  });
  return false;
}

async function performToolEnabledUpdate(
  authorization: ToolAuthorization,
  state: DistributedToolState,
  plugin: PluginItem,
  targetState: boolean,
): Promise<boolean> {
  if (targetState && !(await readSensitivePermission(plugin))) return false;
  const enabledNames = state.pluginsList.value
    .filter((item) => (item.id === plugin.id ? targetState : item.enabled))
    .map((item) => item.id);
  await authorization.updateEnabledTools(enabledNames);
  if (state.pluginsList.value.some((item) => item.id === plugin.id)) {
    plugin.enabled = targetState;
  }
  return true;
}

function reportAuthorizationFailure(
  notificationStore: NotificationStore,
  title: string,
  message: string,
  error: unknown,
): void {
  console.error(`[DistributedTools] ${title}：`, error);
  notificationStore.addNotification({
    type: "error",
    title,
    message,
    toastOnly: true,
  });
}

export function setDistributedToolEnabled(
  authorization: ToolAuthorization,
  state: DistributedToolState,
  notificationStore: NotificationStore,
  plugin: PluginItem,
  targetState: boolean,
): Promise<boolean> {
  if (
    authorization.hasPendingMutation() ||
    distributedResetPending.value ||
    hasAnyDistributedToolMutationPending()
  ) {
    return Promise.resolve(false);
  }
  beginToolMutation(plugin.id);
  return enqueueToolMutation(async () => {
    try {
      return await performToolEnabledUpdate(
        authorization,
        state,
        plugin,
        targetState,
      );
    } catch (error) {
      reportAuthorizationFailure(
        notificationStore,
        "保存工具授权失败",
        "后端未确认本次授权变更，当前状态保持不变。",
        error,
      );
      return false;
    } finally {
      endToolMutation(plugin.id);
    }
  });
}

export function resetDistributedTools(
  authorization: ToolAuthorization,
  notificationStore: NotificationStore,
  loadMetadata: () => Promise<void>,
): Promise<boolean> {
  if (
    pendingResetCount.value > 0 ||
    authorization.hasPendingMutation() ||
    hasAnyDistributedToolMutationPending()
  ) {
    return Promise.resolve(false);
  }
  pendingResetCount.value += 1;
  return enqueueToolMutation(async () => {
    try {
      await authorization.resetDisabledTools();
      await loadMetadata();
      return true;
    } catch (error) {
      reportAuthorizationFailure(
        notificationStore,
        "重置工具授权失败",
        "后端未确认重置操作，请稍后重试。",
        error,
      );
      return false;
    } finally {
      pendingResetCount.value = Math.max(0, pendingResetCount.value - 1);
    }
  });
}
