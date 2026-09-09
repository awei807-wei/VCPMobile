import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import { useNotificationStore } from "./notification";
import { createAssistantAvatarActions } from "./assistantAvatarActions";
import { createAssistantReadActions } from "./assistantReadActions";
import {
  createAssistantUnreadActions,
} from "./assistantUnreadActions";
import { createAssistantWriteActions } from "./assistantWriteActions";

export { ownerUnreadKey } from "./assistantUnreadActions";
export type { AssistantOwnerType } from "./assistantUnreadActions";

export interface Topic {
  id: string;
  name: string;
  createdAt: number;
  locked: boolean;
  unread: boolean;
  unreadCount: number;
  msgCount: number;
  ownerId: string;
  ownerType: string;
}

export interface AgentConfig {
  id: string;
  name: string;
  model: string;
  mobileSystemPrompt?: string;
  temperature?: number;
  contextTokenLimit?: number;
  maxOutputTokens?: number;
  streamOutput?: boolean;
  useTemperature?: boolean;
  avatarCalculatedColor?: string;
  topics?: Topic[];
}

export interface GroupConfig {
  id: string;
  name: string;
  avatarCalculatedColor?: string;
  members: string[];
  mode?: string;
  memberTags?: Record<string, any>;
  groupPrompt?: string;
  invitePrompt?: string;
  useUnifiedModel?: boolean;
  unifiedModel?: string;
  tagMatchMode?: string;
  topics?: Topic[];
  createdAt?: number;
}

export const useAssistantStore = defineStore("assistant", () => {
  const agents = shallowRef<AgentConfig[]>([]);
  const groups = shallowRef<GroupConfig[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const notificationStore = useNotificationStore();

  // 同步完成刷新已集中到 main.ts（window.location.reload），此处无需重复监听
  const unreadCounts = ref<Record<string, number>>({});
  const combinedItems = computed(() => [
    ...agents.value.map((agent) => ({ ...agent, type: "agent" as const })),
    ...groups.value.map((group) => ({ ...group, type: "group" as const })),
  ]);
  const readActions = createAssistantReadActions({
    agents,
    groups,
    unreadCounts,
    loading,
    error,
  });
  const writeActions = createAssistantWriteActions({
    agents,
    groups,
    loading,
    error,
    notificationStore,
  });
  const unreadActions = createAssistantUnreadActions({ unreadCounts });
  const avatarActions = createAssistantAvatarActions({ notificationStore });

  return {
    agents,
    groups,
    combinedItems,
    loading,
    error,
    unreadCounts,
    ...readActions,
    ...writeActions,
    ...avatarActions,
    ...unreadActions,
  };
});
