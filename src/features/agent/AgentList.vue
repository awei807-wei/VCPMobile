<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useLayoutStore } from "../../core/stores/layout";
import { useSettingsStore } from "../../core/stores/settings";
import { useOverlayStore } from "../../core/stores/overlay";
import AgentSwipeItem from "./AgentSwipeItem.vue";
import { ownerSwipeKey, useAgentSwipe } from "./useAgentSwipe";
import { useAgentSorting } from "./useAgentSorting";

const props = defineProps<{
  searchQuery: string;
}>();

const emit = defineEmits<{
  (event: "select-agent", item: any): void;
  (event: "select-group", item: any): void;
}>();

const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();
const layoutStore = useLayoutStore();
const settingsStore = useSettingsStore();
const overlayStore = useOverlayStore();

const compareOrder = (order: string[], leftId: string, rightId: string) => {
  const leftIndex = order.indexOf(leftId);
  const rightIndex = order.indexOf(rightId);
  if (leftIndex === -1 && rightIndex === -1) return 0;
  if (leftIndex === -1) return 1;
  if (rightIndex === -1) return -1;
  return leftIndex - rightIndex;
};

const orderedGroups = computed(() => {
  const groups = assistantStore.groups;
  const order = settingsStore.settings?.groupOrder || [];
  if (order.length === 0) return groups;
  return [...groups].sort((a, b) => compareOrder(order, a.id, b.id));
});

const orderedAgents = computed(() => {
  const agents = assistantStore.agents;
  const order = settingsStore.settings?.agentOrder || [];
  if (order.length === 0) return agents;
  return [...agents].sort((a, b) => compareOrder(order, a.id, b.id));
});

const isSorting = ref(false);
const swipe = useAgentSwipe(isSorting);
const {
  activeSwipeId,
  currentSwipeX,
  isDragging,
  onTouchStart,
  onTouchMove,
  onTouchEnd,
} = swipe;
const sorting = useAgentSorting(
  settingsStore,
  isSorting,
  swipe.isDragging,
  swipe.activeSwipeId,
  swipe.currentSwipeX,
);
const { groupListRef, agentListRef } = sorting;

onMounted(() => {
  sorting.initSortable(orderedGroups, orderedAgents);
});

const goToSettings = (id: string, type: "agent" | "group" = "agent") => {
  swipe.closeActiveSwipe();
  layoutStore.setLeftDrawer(false);
  if (type === "agent") {
    overlayStore.openAgentSettings(id);
  } else {
    overlayStore.openGroupSettings(id);
  }
};

const selectAgent = (agentId: string) => {
  const agent = assistantStore.agents.find((item) => item.id === agentId);
  if (agent) emit("select-agent", { ...agent, type: "agent" });
};

const selectGroup = (groupId: string) => {
  const group = assistantStore.groups.find((item) => item.id === groupId);
  if (group) emit("select-group", { ...group, type: "group" });
};

const filteredCombinedItems = computed(() => {
  const query = props.searchQuery.toLowerCase().trim();
  if (!query) return assistantStore.combinedItems;
  return assistantStore.combinedItems.filter((item) =>
    item.name.toLowerCase().includes(query),
  );
});
</script>

<template>
  <div v-if="assistantStore.loading" class="flex justify-center p-8 opacity-50">
    <svg
      class="animate-spin h-6 w-6 text-primary-text"
      viewBox="0 0 24 24"
      fill="none"
    >
      <circle
        class="opacity-25"
        cx="12"
        cy="12"
        r="10"
        stroke="currentColor"
        stroke-width="4"
      ></circle>
      <path
        class="opacity-75"
        fill="currentColor"
        d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
      ></path>
    </svg>
  </div>
  <div
    v-else-if="filteredCombinedItems.length === 0"
    class="text-center p-8 opacity-30 text-sm"
  >
    未找到助手或群组
  </div>
  <div v-else class="space-y-4">
    <div v-if="assistantStore.groups.length > 0" class="space-y-2">
      <h3
        class="px-2 text-[10px] font-black uppercase tracking-widest opacity-30"
      >
        Agent Groups
      </h3>
      <div
        ref="groupListRef"
        class="space-y-2 px-1"
        :class="{ 'no-swipe': isSorting }"
      >
        <AgentSwipeItem
          v-for="group in orderedGroups.filter(
            (item) =>
              !searchQuery.trim() ||
              item.name
                .toLowerCase()
                .includes(searchQuery.toLowerCase().trim()),
          )"
          :key="group.id"
          owner-type="group"
          :owner-id="group.id"
          :name="group.name"
          :subtitle="`${group.members.length} Members • ${group.mode}`"
          :selected="
            sessionStore.currentSelectedItem?.type === 'group' &&
            sessionStore.currentSelectedItem?.id === group.id
          "
          :show-unread="
            assistantStore.getOwnerUnreadCount(group.id, 'group') === -1 ||
            assistantStore.getOwnerUnreadCount(group.id, 'group') > 0
          "
          :swipe-x="currentSwipeX"
          :swipe-active="activeSwipeId === ownerSwipeKey('group', group.id)"
          :is-dragging="isDragging"
          @select="selectGroup(group.id)"
          @settings="goToSettings(group.id, 'group')"
          @touch-start="onTouchStart($event, ownerSwipeKey('group', group.id))"
          @touch-move="onTouchMove($event, ownerSwipeKey('group', group.id))"
          @touch-end="onTouchEnd($event, ownerSwipeKey('group', group.id))"
        />
      </div>
    </div>

    <div v-if="assistantStore.agents.length > 0" class="space-y-2">
      <h3
        class="px-2 text-[10px] font-black uppercase tracking-widest opacity-30"
      >
        Individual Agents
      </h3>
      <div
        ref="agentListRef"
        class="space-y-2 px-1"
        :class="{ 'no-swipe': isSorting }"
      >
        <AgentSwipeItem
          v-for="agent in orderedAgents.filter(
            (item) =>
              !searchQuery.trim() ||
              item.name
                .toLowerCase()
                .includes(searchQuery.toLowerCase().trim()),
          )"
          :key="agent.id"
          owner-type="agent"
          :owner-id="agent.id"
          :name="agent.name"
          :subtitle="agent.model"
          :selected="
            sessionStore.currentSelectedItem?.type === 'agent' &&
            sessionStore.currentSelectedItem?.id === agent.id
          "
          :show-unread="
            assistantStore.getOwnerUnreadCount(agent.id, 'agent') === -1 ||
            assistantStore.getOwnerUnreadCount(agent.id, 'agent') > 0
          "
          :swipe-x="currentSwipeX"
          :swipe-active="activeSwipeId === ownerSwipeKey('agent', agent.id)"
          :is-dragging="isDragging"
          @select="selectAgent(agent.id)"
          @settings="goToSettings(agent.id, 'agent')"
          @touch-start="onTouchStart($event, ownerSwipeKey('agent', agent.id))"
          @touch-move="onTouchMove($event, ownerSwipeKey('agent', agent.id))"
          @touch-end="onTouchEnd($event, ownerSwipeKey('agent', agent.id))"
        />
      </div>
    </div>
  </div>
</template>
