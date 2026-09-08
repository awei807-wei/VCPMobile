import { invoke } from "@tauri-apps/api/core";
import type { Ref, ShallowRef } from "vue";
import type { useNotificationStore } from "./notification";
import type { AgentConfig, GroupConfig } from "./assistant";

export interface AssistantWriteContext {
  agents: ShallowRef<AgentConfig[]>;
  groups: ShallowRef<GroupConfig[]>;
  loading: Ref<boolean>;
  error: Ref<string | null>;
  notificationStore: ReturnType<typeof useNotificationStore>;
}

async function createAgent(context: AssistantWriteContext, name: string) {
  context.loading.value = true;
  try {
    const newAgent = await invoke<AgentConfig>("create_agent", { name });
    context.notificationStore.addNotification({
      type: "success",
      title: "Agent 创建成功",
      message: `助手 "${name}" 已就绪`,
      toastOnly: true,
    });
    return newAgent;
  } catch (error: any) {
    context.error.value = error.toString();
    throw error;
  } finally {
    context.loading.value = false;
  }
}

async function deleteAgent(context: AssistantWriteContext, id: string) {
  try {
    await invoke("delete_agent", { agentId: id });
    context.agents.value = context.agents.value.filter((agent) => agent.id !== id);
    context.notificationStore.addNotification({
      type: "success",
      title: "Agent 删除成功",
      message: "助手已从列表中移除",
      toastOnly: true,
    });
  } catch (error) {
    console.error("[AssistantStore] Failed to delete agent:", error);
    throw error;
  }
}

async function createGroup(context: AssistantWriteContext, name: string) {
  context.loading.value = true;
  try {
    const newGroup = await invoke<GroupConfig>("create_group", { name });
    context.notificationStore.addNotification({
      type: "success",
      title: "Group 创建成功",
      message: `群组 "${name}" 已创建`,
      toastOnly: true,
    });
    return newGroup;
  } catch (error: any) {
    context.error.value = error.toString();
    throw error;
  } finally {
    context.loading.value = false;
  }
}

async function deleteGroup(context: AssistantWriteContext, id: string) {
  try {
    await invoke("delete_group", { groupId: id });
    context.groups.value = context.groups.value.filter((group) => group.id !== id);
    context.notificationStore.addNotification({
      type: "success",
      title: "Group 删除成功",
      message: "群组已解散",
      toastOnly: true,
    });
  } catch (error) {
    console.error("[AssistantStore] Failed to delete group:", error);
    throw error;
  }
}

async function saveAgent(context: AssistantWriteContext, agent: AgentConfig) {
  try {
    await invoke("save_agent_config", { agent });
    const index = context.agents.value.findIndex((item) => item.id === agent.id);
    if (index !== -1) {
      const updated = [...context.agents.value];
      updated[index] = {
        ...updated[index],
        name: agent.name,
        model: agent.model,
        avatarCalculatedColor:
          agent.avatarCalculatedColor || updated[index].avatarCalculatedColor,
      };
      context.agents.value = updated;
    }
    context.notificationStore.addNotification({
      type: "success",
      title: "Agent 配置保存成功",
      message: "助手的最新设置已同步到核心",
      toastOnly: true,
    });
  } catch (error: any) {
    context.error.value = error.toString();
    throw error;
  }
}

async function saveGroup(context: AssistantWriteContext, group: GroupConfig) {
  try {
    await invoke("save_group_config", { group });
    const index = context.groups.value.findIndex((item) => item.id === group.id);
    if (index !== -1) {
      const updated = [...context.groups.value];
      updated[index] = {
        ...updated[index],
        name: group.name,
        members: group.members,
        avatarCalculatedColor:
          group.avatarCalculatedColor || updated[index].avatarCalculatedColor,
      };
      context.groups.value = updated;
    }
    context.notificationStore.addNotification({
      type: "success",
      title: "Group 配置保存成功",
      message: "群组设置已更新",
      toastOnly: true,
    });
  } catch (error: any) {
    context.error.value = error.toString();
    throw error;
  }
}

export function createAssistantWriteActions(context: AssistantWriteContext) {
  return {
    createAgent: (name: string) => createAgent(context, name),
    deleteAgent: (id: string) => deleteAgent(context, id),
    createGroup: (name: string) => createGroup(context, name),
    deleteGroup: (id: string) => deleteGroup(context, id),
    saveAgent: (agent: AgentConfig) => saveAgent(context, agent),
    saveGroup: (group: GroupConfig) => saveGroup(context, group),
  };
}
