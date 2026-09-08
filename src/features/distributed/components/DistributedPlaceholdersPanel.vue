<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useNotificationStore } from "../../../core/stores/notification";
import { useDistributedTools } from "../composables/useDistributedTools";

const notificationStore = useNotificationStore();
const { placeholdersList, loadPluginsMetadata, clear } = useDistributedTools();
const searchQuery = ref("");

const filteredPlaceholders = computed(() => {
  const query = searchQuery.value.trim().toLowerCase();
  if (!query) return placeholdersList.value;
  return placeholdersList.value.filter(
    (item) =>
      item.macro.toLowerCase().includes(query) ||
      item.name.toLowerCase().includes(query) ||
      item.description.toLowerCase().includes(query),
  );
});

async function refreshPlaceholders(): Promise<void> {
  try {
    await loadPluginsMetadata();
  } catch (error) {
    console.error("[DistributedPlaceholders] 加载占位符失败：", error);
    notificationStore.addNotification({
      type: "error",
      title: "加载占位符失败",
      message: "无法确认当前工具授权状态。",
      toastOnly: true,
    });
  }
}

async function copyPlaceholder(macro: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(macro);
    notificationStore.addNotification({
      type: "success",
      title: "占位符宏已复制",
      message: macro,
      toastOnly: true,
    });
  } catch (error) {
    console.error("[DistributedPlaceholders] 复制失败：", error);
  }
}

onMounted(() => void refreshPlaceholders());
onUnmounted(clear);
</script>

<template>
  <div class="flex flex-col h-full">
    <div class="px-4 py-3 shrink-0 border-b border-black/5 dark:border-white/5">
      <div class="relative">
        <input
          v-model="searchQuery"
          type="text"
          placeholder="搜索占位符宏（例如 CPU、GPS）"
          class="w-full bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 rounded-xl px-3.5 py-2.5 text-xs text-primary-text focus:outline-none focus:border-[var(--highlight-text)]/50"
        />
        <button
          v-if="searchQuery"
          class="absolute right-3 top-1/2 -translate-y-1/2 opacity-50 text-[10px]"
          @click="searchQuery = ''"
        >
          清除
        </button>
      </div>
      <p class="mt-2 text-[10px] opacity-50">点击占位符卡片复制宏；读取动作只在插件页的显式按钮触发。</p>
    </div>
    <div class="flex-1 overflow-y-auto px-4 py-4 space-y-3 no-rubber-band">
      <button
        v-for="item in filteredPlaceholders"
        :key="item.macro"
        class="w-full text-left bg-black/2 dark:bg-white/2 border border-black/5 dark:border-white/5 rounded-2xl p-4 flex flex-col gap-2 hover:border-black/10 dark:hover:border-white/10"
        @click="copyPlaceholder(item.macro)"
      >
        <div class="flex items-center justify-between gap-2">
          <span class="text-xs font-bold">{{ item.name }}</span>
          <span class="font-mono text-[9px] bg-black/10 dark:bg-white/10 px-2 py-0.5 rounded text-[var(--highlight-text)] select-all">{{ item.macro }}</span>
        </div>
        <p class="text-[10px] opacity-50">{{ item.description }}</p>
      </button>
      <div v-if="!filteredPlaceholders.length" class="text-center py-12 text-xs opacity-50">暂无可用占位符</div>
    </div>
  </div>
</template>
