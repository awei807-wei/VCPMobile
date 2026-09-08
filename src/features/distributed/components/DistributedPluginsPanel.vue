<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useNotificationStore } from "../../../core/stores/notification";
import { useDistributedRootAccess } from "../composables/useDistributedRootAccess";
import {
  useDistributedTools,
  type PluginItem,
} from "../composables/useDistributedTools";

const notificationStore = useNotificationStore();
const {
  pluginsList,
  pluginLoading,
  pluginData,
  pluginFoldBlocks,
  selectedFoldBlockIdx,
  loadPluginsMetadata,
  setToolEnabled,
  resetDisabledTools,
  loadPluginDetails,
  selectFoldBlock,
  authorizationLoading,
  authorizationResetPending,
  isToolMutationPending,
} = useDistributedTools();
const {
  rootGranted,
  check: checkRoot,
  openManager,
  dispose: disposeRoot,
} = useDistributedRootAccess();

const searchQuery = ref("");
const expandedPluginId = ref<string | null>(null);

const filteredPlugins = computed(() => {
  const query = searchQuery.value.trim().toLowerCase();
  if (!query) return pluginsList.value;
  return pluginsList.value.filter(
    (plugin) =>
      plugin.name.toLowerCase().includes(query) ||
      plugin.englishName.toLowerCase().includes(query) ||
      plugin.description.toLowerCase().includes(query),
  );
});

function notifyLoadFailure(error: unknown): void {
  console.error("[DistributedPlugins] 加载工具列表失败：", error);
  notificationStore.addNotification({
    type: "error",
    title: "加载工具列表失败",
    message: "无法确认工具授权状态，工具已按安全策略关闭。",
    toastOnly: true,
  });
}

async function refreshPlugins(): Promise<void> {
  try {
    await loadPluginsMetadata();
  } catch (error) {
    notifyLoadFailure(error);
  }
}

async function toggleTool(plugin: PluginItem): Promise<void> {
  if (authorizationLoading.value || isToolMutationPending(plugin.id)) return;
  const changed = await setToolEnabled(plugin, !plugin.enabled);
  if (changed && !plugin.enabled && expandedPluginId.value === plugin.id) {
    expandedPluginId.value = null;
  }
}

function togglePlugin(plugin: PluginItem): void {
  expandedPluginId.value =
    expandedPluginId.value === plugin.id ? null : plugin.id;
}

async function readPlugin(plugin: PluginItem): Promise<void> {
  if (!plugin.enabled) {
    notificationStore.addNotification({
      type: "warning",
      title: "工具已关闭",
      message: `${plugin.name} 未获授权，无法读取实时数据。`,
      toastOnly: true,
    });
    return;
  }
  await loadPluginDetails(plugin);
}

async function copyText(value: string): Promise<void> {
  if (!value) return;
  try {
    await navigator.clipboard.writeText(value);
    notificationStore.addNotification({
      type: "success",
      title: "占位符宏已复制",
      message: value,
      toastOnly: true,
    });
  } catch (error) {
    console.error("[DistributedPlugins] 复制占位符失败：", error);
  }
}

onMounted(() => void refreshPlugins());
onUnmounted(disposeRoot);
</script>

<template>
  <div class="flex flex-col h-full">
    <div
      class="px-4 py-3 shrink-0 border-b border-black/5 dark:border-white/5 space-y-2"
    >
      <div class="relative">
        <input
          v-model="searchQuery"
          type="text"
          placeholder="搜索分布式插件（例如 CPU、定位）"
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
      <div class="flex items-center justify-between gap-2">
        <span class="text-[10px] opacity-50"
          >授权后工具才会注册到分布式连接。</span
        >
        <button
          class="px-2.5 py-1 bg-black/10 dark:bg-white/10 rounded-lg text-[9px] font-bold disabled:opacity-40"
          :disabled="authorizationLoading || authorizationResetPending"
          @click="resetDisabledTools"
        >
          重置授权
        </button>
      </div>
    </div>

    <div class="flex-1 overflow-y-auto px-4 py-4 space-y-3 no-rubber-band">
      <div
        v-for="plugin in filteredPlugins"
        :key="plugin.id"
        class="border border-black/5 dark:border-white/5 rounded-2xl overflow-hidden transition-all"
        :class="[
          expandedPluginId === plugin.id ? 'bg-black/5 dark:bg-white/5' : '',
          !plugin.enabled ? 'opacity-60' : '',
        ]"
      >
        <div
          class="p-4 flex items-center justify-between cursor-pointer select-none"
          @click="togglePlugin(plugin)"
        >
          <div class="flex items-center gap-2.5 min-w-0">
            <div
              class="w-7 h-7 shrink-0 rounded-lg bg-black/5 dark:bg-white/10 flex items-center justify-center text-[10px] font-bold"
            >
              {{ plugin.englishName.slice(0, 2) }}
            </div>
            <div class="flex flex-col min-w-0">
              <div class="flex items-baseline gap-1.5 flex-wrap">
                <span class="text-xs font-bold">{{ plugin.name }}</span>
                <span class="text-[8px] font-mono opacity-40 uppercase">{{
                  plugin.englishName
                }}</span>
              </div>
              <span class="text-[10px] opacity-50 mt-0.5 line-clamp-1">{{
                plugin.description
              }}</span>
            </div>
          </div>
          <div class="flex items-center gap-2 shrink-0">
            <button
              class="w-9 h-5 rounded-full p-0.5 transition-colors flex items-center disabled:opacity-40"
              :disabled="
                authorizationLoading || isToolMutationPending(plugin.id)
              "
              :class="
                plugin.enabled
                  ? 'bg-[var(--highlight-text)]/50 justify-end'
                  : 'bg-black/15 dark:bg-white/15 justify-start'
              "
              :aria-label="`${plugin.name}授权开关`"
              @click.stop="toggleTool(plugin)"
            >
              <span
                class="w-4 h-4 rounded-full bg-white dark:bg-black shadow-sm"
              ></span>
            </button>
            <span
              class="text-[7px] font-mono uppercase px-1.5 py-0.5 rounded border border-black/10 dark:border-white/10"
            >
              {{ plugin.type }}
            </span>
            <span class="text-xs opacity-40">{{
              expandedPluginId === plugin.id ? "⌃" : "⌄"
            }}</span>
          </div>
        </div>

        <div
          v-if="expandedPluginId === plugin.id"
          class="border-t border-black/5 dark:border-white/5 p-4 bg-black/5 dark:bg-white/10 space-y-3"
        >
          <div class="text-[10px] opacity-70 leading-relaxed">
            <span
              class="font-bold opacity-50 text-[8px] uppercase tracking-wider block mb-1"
              >功能描述</span
            >
            {{ plugin.description || "暂无描述" }}
          </div>
          <div
            v-if="plugin.requiresRoot"
            class="bg-amber-500/10 border border-amber-500/20 p-2.5 rounded-xl flex items-center justify-between text-[10px]"
          >
            <span class="text-amber-800 dark:text-amber-400">{{
              rootGranted === true
                ? "Root 已授权"
                : rootGranted === false
                  ? "未获得 Root，数据将降级"
                  : "此工具可能需要 Root 权限"
            }}</span>
            <button
              v-if="rootGranted === null"
              class="text-[8px] font-bold"
              @click.stop="checkRoot"
            >
              检测 Root
            </button>
            <button
              v-else-if="rootGranted === false"
              class="text-[8px] font-bold"
              @click.stop="openManager"
            >
              跳转授权
            </button>
          </div>
          <div
            class="flex justify-between items-center text-[9px] uppercase font-bold opacity-50"
          >
            <span>{{
              plugin.type === "streaming" ? "实时遥测数据" : "调用说明"
            }}</span>
            <button
              v-if="plugin.type === 'streaming'"
              class="px-2 py-1 bg-black/10 dark:bg-white/10 rounded-md text-[8px] disabled:opacity-40"
              :disabled="!plugin.enabled || pluginLoading[plugin.id]"
              @click.stop="readPlugin(plugin)"
            >
              {{ pluginLoading[plugin.id] ? "读取中…" : "读取实时数据" }}
            </button>
          </div>
          <div
            v-if="!plugin.enabled"
            class="text-[10px] text-amber-600 dark:text-amber-400"
          >
            当前仅展示说明；显式授权后才能读取数据。
          </div>
          <div
            v-if="pluginLoading[plugin.id]"
            class="text-center py-4 opacity-50 text-xs"
          >
            正在读取设备数据…
          </div>
          <div v-else class="space-y-2">
            <div
              v-if="pluginFoldBlocks[plugin.id]?.length"
              class="flex flex-wrap gap-1.5"
            >
              <button
                v-for="(block, index) in pluginFoldBlocks[plugin.id]"
                :key="index"
                class="px-2.5 py-1 text-[9px] font-bold rounded-lg border"
                :class="
                  selectedFoldBlockIdx[plugin.id] === index
                    ? 'text-[var(--highlight-text)] border-[var(--highlight-text)]/30'
                    : 'border-transparent opacity-60'
                "
                @click.stop="selectFoldBlock(plugin.id, index)"
              >
                {{ block.desc }}（{{ block.threshold }}）
              </button>
            </div>
            <pre
              class="rounded-xl bg-black/5 dark:bg-black/40 border border-black/5 dark:border-white/5 p-3.5 text-[10px] font-mono whitespace-pre-wrap leading-relaxed break-all"
              >{{
                pluginData[plugin.id] ||
                (plugin.type === "streaming"
                  ? "尚未读取实时数据"
                  : "展开后显示调用说明")
              }}</pre
            >
          </div>
          <button
            v-if="plugin.placeholder"
            class="text-[10px] font-mono text-[var(--highlight-text)] border border-[var(--highlight-text)]/20 px-1.5 py-0.5 rounded"
            @click.stop="copyText(plugin.placeholder)"
          >
            {{ plugin.placeholder }}
          </button>
        </div>
      </div>
      <div
        v-if="!filteredPlugins.length"
        class="text-center py-12 text-xs opacity-50"
      >
        暂无匹配工具
      </div>
    </div>
  </div>
</template>
