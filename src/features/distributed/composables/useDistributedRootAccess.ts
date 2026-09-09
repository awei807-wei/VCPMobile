import { ref } from "vue";
import {
  checkRootAccess,
  launchRootManager,
} from "../../../../src-tauri/plugins/vcp-mobile/guest-js";
import { useNotificationStore } from "../../../core/stores/notification";

const rootGranted = ref<boolean | null>(null);
let checkTimer: ReturnType<typeof setTimeout> | null = null;

export function useDistributedRootAccess() {
  const notificationStore = useNotificationStore();

  async function check(): Promise<boolean> {
    try {
      const result = await checkRootAccess();
      rootGranted.value = result.isRoot === true;
    } catch (error) {
      console.error("[DistributedRoot] 检测 Root 权限失败：", error);
      rootGranted.value = false;
    }
    return rootGranted.value === true;
  }

  async function openManager(): Promise<void> {
    try {
      const result = await launchRootManager();
      if (result.success) {
        notificationStore.addNotification({
          type: "success",
          title: "启动成功",
          message: `已启动 ${result.manager || "Root 管理器"}，授权后返回应用重新检测。`,
          toastOnly: true,
        });
        if (checkTimer) clearTimeout(checkTimer);
        checkTimer = setTimeout(() => void check(), 3000);
      } else {
        notificationStore.addNotification({
          type: "warning",
          title: "未找到授权管理器",
          message: result.message || "未检测到可用的 Root 管理器应用。",
          toastOnly: true,
        });
      }
    } catch (error) {
      console.error("[DistributedRoot] 启动 Root 管理器失败：", error);
      notificationStore.addNotification({
        type: "error",
        title: "启动管理器失败",
        message: String(error),
        toastOnly: true,
      });
    }
  }

  function dispose(): void {
    if (checkTimer) {
      clearTimeout(checkTimer);
      checkTimer = null;
    }
  }

  return { rootGranted, check, openManager, dispose };
}
