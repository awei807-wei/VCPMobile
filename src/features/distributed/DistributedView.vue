<script setup lang="ts">
import { ref, watch } from "vue";
import SlidePage from "../../components/ui/SlidePage.vue";
import DistributedConnectionPanel from "./components/DistributedConnectionPanel.vue";
import DistributedPluginsPanel from "./components/DistributedPluginsPanel.vue";
import DistributedPlaceholdersPanel from "./components/DistributedPlaceholdersPanel.vue";
import { useDistributedTools } from "./composables/useDistributedTools";

const props = withDefaults(
  defineProps<{
    isOpen?: boolean;
    zIndex?: number;
  }>(),
  { isOpen: false, zIndex: 50 },
);

const emit = defineEmits<{ close: [] }>();
const activeTab = ref<"connection" | "plugins" | "placeholders">("connection");
const { pluginsList } = useDistributedTools();
const tabs = [
  { id: "connection", label: "基础连接" },
  { id: "plugins", label: "插件列表" },
  { id: "placeholders", label: "占位符" },
] as const;

function selectTab(tab: (typeof tabs)[number]["id"]): void {
  activeTab.value = tab;
}

watch(
  () => props.isOpen,
  (isOpen) => {
    if (isOpen) activeTab.value = "connection";
  },
);
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div
      class="distributed-view flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto"
    >
      <header
        class="px-4 py-3 flex items-center justify-between border-b border-black/5 dark:border-white/5 pt-[calc(var(--vcp-safe-top,24px)+12px)] shrink-0"
      >
        <div class="flex items-baseline gap-2 flex-wrap">
          <h2 class="text-xl font-bold tracking-tight shrink-0">
            分布式设备面板
          </h2>
          <span
            class="text-[8px] font-mono opacity-40 uppercase tracking-wider shrink-0"
            >Distributed Panel // Client V2</span
          >
        </div>
        <button
          class="p-2 opacity-70 rounded-xl hover:bg-black/5 dark:hover:bg-white/5"
          aria-label="关闭分布式设备面板"
          @click="emit('close')"
        >
          <svg
            width="22"
            height="22"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
          >
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </header>

      <nav
        class="px-4 py-2 shrink-0 border-b border-black/5 dark:border-white/5 flex gap-2"
        aria-label="分布式面板分类"
      >
        <button
          v-for="tab in tabs"
          :key="tab.id"
          class="flex-1 py-1.5 text-xs font-bold rounded-lg transition-all"
          :class="
            activeTab === tab.id
              ? 'bg-black/5 dark:bg-white/5 text-[var(--highlight-text)] border border-[var(--highlight-text)]/20'
              : 'opacity-60 hover:opacity-80'
          "
          @click="selectTab(tab.id)"
        >
          {{ tab.label }}
        </button>
      </nav>

      <div class="flex-1 overflow-y-auto no-rubber-band relative">
        <DistributedConnectionPanel
          v-if="activeTab === 'connection'"
          :tool-count="pluginsList.length"
        />
        <DistributedPluginsPanel v-else-if="activeTab === 'plugins'" />
        <DistributedPlaceholdersPanel v-else />
      </div>
    </div>
  </SlidePage>
</template>

<style scoped>
.distributed-view {
  background-color: var(--primary-bg);
}
</style>
