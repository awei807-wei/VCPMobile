import { invoke } from "@tauri-apps/api/core";
import type { useNotificationStore } from "./notification";

export type AssistantAvatarOwnerType = "agent" | "group" | "user";

export interface AssistantAvatarContext {
  notificationStore: ReturnType<typeof useNotificationStore>;
}

async function saveAvatar(
  context: AssistantAvatarContext,
  ownerType: AssistantAvatarOwnerType,
  ownerId: string,
  mimeType: string,
  imageData: number[],
) {
  try {
    const hash = await invoke<string>("save_avatar_data", {
      ownerType,
      ownerId,
      mimeType,
      imageData,
    });
    const label = ownerType === "agent" ? "Agent" : ownerType === "group" ? "Group" : "用户";
    context.notificationStore.addNotification({
      type: "success",
      title: `${label} 头像更新成功`,
      message: "新头像已生效",
      toastOnly: true,
    });
    return hash;
  } catch (error: any) {
    console.error(`[AssistantStore] Failed to save avatar for ${ownerType}:`, error);
    throw error;
  }
}

export function createAssistantAvatarActions(context: AssistantAvatarContext) {
  return {
    saveAvatar: (
      ownerType: AssistantAvatarOwnerType,
      ownerId: string,
      mimeType: string,
      imageData: number[],
    ) => saveAvatar(context, ownerType, ownerId, mimeType, imageData),
  };
}
