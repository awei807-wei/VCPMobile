import { ref, type ComputedRef, type Ref } from "vue";
import Sortable from "sortablejs";
import type { useSettingsStore } from "../../core/stores/settings";

interface SortableItem {
  id: string;
}

type SettingsStore = ReturnType<typeof useSettingsStore>;

interface SortableListOptions {
  listRef: Ref<HTMLElement | null>;
  items: ComputedRef<ReadonlyArray<SortableItem>>;
  orderKey: "groupOrder" | "agentOrder";
}

const resetSwipeState = (
  isDragging: Ref<boolean>,
  activeSwipeId: Ref<string | null>,
  currentSwipeX: Ref<number>,
) => {
  isDragging.value = false;
  activeSwipeId.value = null;
  currentSwipeX.value = 0;
};

const registerSortableList = (
  options: SortableListOptions,
  settingsStore: SettingsStore,
  isSorting: Ref<boolean>,
  isDragging: Ref<boolean>,
  activeSwipeId: Ref<string | null>,
  currentSwipeX: Ref<number>,
) => {
  const { listRef, items, orderKey } = options;
  if (!listRef.value) return;
  Sortable.create(listRef.value, {
    animation: 150,
    handle: ".drag-handle",
    delay: 200,
    delayOnTouchOnly: true,
    touchStartThreshold: 3,
    direction: "vertical",
    forceFallback: true,
    fallbackOnBody: true,
    ghostClass: "opacity-50",
    onChoose: () => {
      isSorting.value = true;
      resetSwipeState(isDragging, activeSwipeId, currentSwipeX);
    },
    onUnchoose: () => {
      isSorting.value = false;
    },
    onStart: () => {
      isSorting.value = true;
    },
    onEnd: (event) => {
      isSorting.value = false;
      const newOrder = items.value.map((item) => item.id);
      const [movedItem] = newOrder.splice(event.oldIndex!, 1);
      newOrder.splice(event.newIndex!, 0, movedItem);
      void settingsStore.updateSettings({ [orderKey]: newOrder });
    },
  });
};

const initSortableLists = (
  groupListRef: Ref<HTMLElement | null>,
  agentListRef: Ref<HTMLElement | null>,
  orderedGroups: ComputedRef<ReadonlyArray<SortableItem>>,
  orderedAgents: ComputedRef<ReadonlyArray<SortableItem>>,
  settingsStore: SettingsStore,
  isSorting: Ref<boolean>,
  isDragging: Ref<boolean>,
  activeSwipeId: Ref<string | null>,
  currentSwipeX: Ref<number>,
) => {
  registerSortableList(
    { listRef: groupListRef, items: orderedGroups, orderKey: "groupOrder" },
    settingsStore,
    isSorting,
    isDragging,
    activeSwipeId,
    currentSwipeX,
  );
  registerSortableList(
    { listRef: agentListRef, items: orderedAgents, orderKey: "agentOrder" },
    settingsStore,
    isSorting,
    isDragging,
    activeSwipeId,
    currentSwipeX,
  );
};

export function useAgentSorting(
  settingsStore: SettingsStore,
  isSorting: Ref<boolean>,
  isDragging: Ref<boolean>,
  activeSwipeId: Ref<string | null>,
  currentSwipeX: Ref<number>,
) {
  const groupListRef = ref<HTMLElement | null>(null);
  const agentListRef = ref<HTMLElement | null>(null);
  const initSortable = (
    orderedGroups: ComputedRef<ReadonlyArray<SortableItem>>,
    orderedAgents: ComputedRef<ReadonlyArray<SortableItem>>,
  ) => {
    initSortableLists(
      groupListRef,
      agentListRef,
      orderedGroups,
      orderedAgents,
      settingsStore,
      isSorting,
      isDragging,
      activeSwipeId,
      currentSwipeX,
    );
  };

  return {
    groupListRef,
    agentListRef,
    isSorting,
    initSortable,
  };
}
