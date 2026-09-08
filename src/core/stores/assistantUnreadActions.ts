import { invoke } from "@tauri-apps/api/core";
import type { Ref } from "vue";

export type AssistantOwnerType = "agent" | "group";

/** Stable composite key for owner badges across Agent and Group namespaces. */
export function ownerUnreadKey(
  ownerType: AssistantOwnerType,
  ownerId: string,
): string {
  return `${ownerType}:${encodeURIComponent(ownerId)}`;
}

export interface AssistantUnreadContext {
  unreadCounts: Ref<Record<string, number>>;
}

async function refreshUnreadCounts(context: AssistantUnreadContext) {
  try {
    const counts = await invoke<Record<string, number>>("get_unread_counts");
    context.unreadCounts.value = counts;
  } catch (error) {
    console.error("[AssistantStore] Failed to refresh unread counts:", error);
  }
}

function applyOwnerUnreadCount(
  context: AssistantUnreadContext,
  ownerId: string,
  ownerType: AssistantOwnerType,
  unreadCount: number,
) {
  if (!Number.isSafeInteger(unreadCount) || unreadCount < -1) return;
  context.unreadCounts.value = {
    ...context.unreadCounts.value,
    [ownerUnreadKey(ownerType, ownerId)]: unreadCount,
  };
}

async function refreshOwnerUnreadCount(
  context: AssistantUnreadContext,
  ownerId: string,
  ownerType: AssistantOwnerType,
) {
  const state = await invoke<{
    ownerId: string;
    ownerType: AssistantOwnerType;
    unreadCount: number;
  }>("get_owner_unread_count", { ownerId, ownerType });
  if (state.ownerId !== ownerId || state.ownerType !== ownerType) {
    throw new Error("后端返回的 owner 未读聚合身份无效");
  }
  applyOwnerUnreadCount(context, ownerId, ownerType, state.unreadCount);
}

function getOwnerUnreadCount(
  context: AssistantUnreadContext,
  ownerId: string,
  ownerType: AssistantOwnerType,
) {
  return context.unreadCounts.value[ownerUnreadKey(ownerType, ownerId)] ?? 0;
}

export function createAssistantUnreadActions(context: AssistantUnreadContext) {
  return {
    refreshUnreadCounts: () => refreshUnreadCounts(context),
    applyOwnerUnreadCount: (
      ownerId: string,
      ownerType: AssistantOwnerType,
      unreadCount: number,
    ) => applyOwnerUnreadCount(context, ownerId, ownerType, unreadCount),
    refreshOwnerUnreadCount: (ownerId: string, ownerType: AssistantOwnerType) =>
      refreshOwnerUnreadCount(context, ownerId, ownerType),
    getOwnerUnreadCount: (ownerId: string, ownerType: AssistantOwnerType) =>
      getOwnerUnreadCount(context, ownerId, ownerType),
  };
}
