import { invoke } from "@tauri-apps/api/core";
import type { Ref, ShallowRef } from "vue";
import type { AgentConfig, GroupConfig } from "./assistant";

export interface AssistantSnapshot {
  agents: AgentConfig[];
  groups: GroupConfig[];
  unreadCounts: Record<string, number>;
}

export interface AssistantReadContext {
  agents: ShallowRef<AgentConfig[]>;
  groups: ShallowRef<GroupConfig[]>;
  unreadCounts: Ref<Record<string, number>>;
  loading: Ref<boolean>;
  error: Ref<string | null>;
}

async function fetchAgents(context: AssistantReadContext) {
  context.loading.value = true;
  context.error.value = null;
  const startTime = Date.now();
  try {
    console.log("[Profile] fetchAgents invoking get_assistants_snapshot...");
    const snapshot = await invoke<AssistantSnapshot>("get_assistants_snapshot");
    context.agents.value = snapshot.agents;
    context.unreadCounts.value = snapshot.unreadCounts;
    console.log(`[Profile] fetchAgents finished in ${Date.now() - startTime}ms`);
  } catch (error: any) {
    context.error.value = error.toString();
    console.error("[AssistantStore] fetchAgents failed:", error);
    throw error;
  } finally {
    context.loading.value = false;
  }
}

async function fetchGroups(context: AssistantReadContext) {
  context.loading.value = true;
  context.error.value = null;
  const startTime = Date.now();
  try {
    console.log("[Profile] fetchGroups invoking get_assistants_snapshot...");
    const snapshot = await invoke<AssistantSnapshot>("get_assistants_snapshot");
    context.groups.value = snapshot.groups;
    context.unreadCounts.value = snapshot.unreadCounts;
    console.log(`[Profile] fetchGroups finished in ${Date.now() - startTime}ms`);
  } catch (error: any) {
    context.error.value = error.toString();
    console.error("[AssistantStore] fetchGroups failed:", error);
    throw error;
  } finally {
    context.loading.value = false;
  }
}

async function fetchAgentsAndGroups(context: AssistantReadContext) {
  context.loading.value = true;
  context.error.value = null;
  const startTime = Date.now();
  try {
    console.log("[Profile] invoke('get_assistants_snapshot') starting...");
    const snapshot = await invoke<AssistantSnapshot>("get_assistants_snapshot");
    console.log(
      `[Profile] invoke('get_assistants_snapshot') resolved in ${Date.now() - startTime}ms`,
    );
    context.agents.value = snapshot.agents;
    context.groups.value = snapshot.groups;
    context.unreadCounts.value = snapshot.unreadCounts;
    console.log(
      `[Profile] fetchAgentsAndGroups finished in ${Date.now() - startTime}ms`,
    );
  } catch (error: any) {
    context.error.value = error.toString();
    console.error("[AssistantStore] fetchAgentsAndGroups failed:", error);
    throw error;
  } finally {
    context.loading.value = false;
  }
}

export function createAssistantReadActions(context: AssistantReadContext) {
  return {
    fetchAgents: () => fetchAgents(context),
    fetchGroups: () => fetchGroups(context),
    fetchAgentsAndGroups: () => fetchAgentsAndGroups(context),
  };
}
