<script setup lang="ts">
import { computed } from "vue";
import {
  AlertTriangle,
  CheckCircle2,
  CircleDot,
  Copy,
  LoaderCircle,
  RotateCcw,
  Square,
  X,
} from "lucide-vue-next";
import SlidePage from "../../components/ui/SlidePage.vue";
import SyncLogBrowserCore from "../../features/settings/components/SyncLogBrowserCore.vue";
import SyncLivePanel from "./components/SyncLivePanel.vue";
import { useSyncSessionStore } from "../../core/stores/syncSession";
import { useOverlayStore } from "../../core/stores/overlay";

interface Props {
  zIndex?: number;
}

const props = defineProps<Props>();
const store = useSyncSessionStore();
const overlayStore = useOverlayStore();

const statusLabel = computed(
  () =>
    ({
      idle: "未开始",
      connecting: "连接与握手",
      connected: "同步中",
      retrying: "自动重试",
      stopping: "停止中",
      stopped: "已停止",
      error: "同步失败",
      completed_with_warnings: "完成，有警告",
      completed: "已完成",
    })[store.status],
);
const statusIcon = computed(() => {
  if (
    ["connecting", "connected", "retrying", "stopping"].includes(store.status)
  )
    return LoaderCircle;
  if (store.status === "completed") return CheckCircle2;
  if (store.status === "completed_with_warnings" || store.status === "error")
    return AlertTriangle;
  if (store.status === "stopped") return Square;
  return store.status === "idle" ? CircleDot : RotateCcw;
});
const statusColor = computed(
  () =>
    ({
      idle: "text-white/45",
      connecting: "text-yellow-300",
      connected: "text-blue-300",
      retrying: "text-yellow-300",
      stopping: "text-white/50",
      stopped: "text-white/50",
      error: "text-red-300",
      completed_with_warnings: "text-yellow-300",
      completed: "text-green-300",
    })[store.status],
);
const isSyncing = computed(() => store.isActive);
const retryLabel = computed(() =>
  store.status === "completed_with_warnings"
    ? "处理后重新同步"
    : store.terminalError?.retryAction === "after_user_action"
      ? "已处理，重新同步"
      : "重新同步",
);
const canRetry = computed(
  () =>
    store.status === "completed_with_warnings" ||
    (store.status === "error" &&
      ["manual", "after_user_action"].includes(
        store.terminalError?.retryAction ?? "never",
      )),
);

const handleClose = () => overlayStore.closeSyncSession();
</script>

<template>
  <SlidePage :is-open="store.isOpen" :z-index="props.zIndex">
    <div
      class="vcp-safe-inline fixed inset-0 flex flex-col overflow-hidden bg-[#0a0f14] text-white"
      :class="{ 'pointer-events-none': !store.isOpen }"
    >
      <header class="shrink-0 border-b border-white/8">
        <div
          class="flex items-center gap-2 px-4 pt-[calc(var(--vcp-safe-top,0px)+6px)] pb-2"
        >
          <div class="flex min-w-0 flex-1 items-center gap-3">
            <div class="flex shrink-0 items-center gap-2" :class="statusColor">
              <component
                :is="statusIcon"
                :size="15"
                :class="{ 'animate-spin': isSyncing }"
                aria-hidden="true"
              />
              <span class="text-xs font-bold tracking-widest">{{
                statusLabel
              }}</span>
            </div>
            <nav
              class="flex min-w-0 items-center gap-1"
              role="tablist"
              aria-label="同步视图"
            >
              <button
                id="sync-live-tab"
                type="button"
                role="tab"
                aria-controls="sync-live-panel"
                :aria-selected="store.activeTab === 'live'"
                :disabled="isSyncing && store.activeTab !== 'live'"
                class="min-h-11 min-w-16 border-b-2 px-2 text-xs font-bold tracking-wide transition-colors active:text-white disabled:opacity-25 focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-300"
                :class="
                  store.activeTab === 'live'
                    ? 'border-blue-400/60 text-white/90'
                    : 'border-transparent text-white/40'
                "
                @click="store.switchTab('live')"
              >
                实时同步
              </button>
              <button
                id="sync-history-tab"
                type="button"
                role="tab"
                aria-controls="sync-history-panel"
                :aria-selected="store.activeTab === 'history'"
                :disabled="isSyncing"
                class="min-h-11 min-w-16 border-b-2 px-2 text-xs font-bold tracking-wide transition-colors active:text-white disabled:opacity-25 focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-300"
                :class="
                  store.activeTab === 'history'
                    ? 'border-blue-400/60 text-white/90'
                    : 'border-transparent text-white/40'
                "
                @click="store.switchTab('history')"
              >
                历史日志
              </button>
            </nav>
          </div>
          <button
            v-if="store.canDismiss"
            type="button"
            aria-label="关闭同步面板"
            class="-mr-2 flex h-11 w-11 shrink-0 items-center justify-center text-gray-400 transition-colors active:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-300"
            @click="handleClose"
          >
            <X :size="20" aria-hidden="true" />
          </button>
        </div>
      </header>

      <div class="flex-1 min-h-0 overflow-hidden">
        <SyncLivePanel v-if="store.activeTab === 'live'" />
        <div
          v-else
          id="sync-history-panel"
          role="tabpanel"
          aria-labelledby="sync-history-tab"
          class="h-full flex flex-col overflow-hidden"
        >
          <SyncLogBrowserCore />
        </div>
      </div>

      <footer
        class="flex shrink-0 items-center justify-between border-t border-white/5 px-4 py-2 pb-[calc(var(--vcp-safe-bottom,48px)+4px)]"
      >
        <div
          class="text-[9px] font-bold uppercase tracking-[0.2em] text-white/35"
        >
          <span v-if="store.status === 'idle'">选择上方操作以继续</span>
          <span v-else-if="store.status === 'connecting'"
            >正在建立同步通道</span
          >
          <span v-else-if="store.status === 'connected'">同步进行中</span>
          <span v-else-if="store.status === 'retrying'">正在自动重试</span>
          <span v-else-if="store.status === 'stopping'">正在停止同步</span>
          <span v-else-if="store.status === 'stopped'">同步已停止</span>
          <span v-else-if="store.status === 'completed'">同步已完成</span>
          <span v-else-if="store.status === 'completed_with_warnings'"
            >同步完成，部分信息需处理</span
          >
          <span v-else>同步未完成</span>
        </div>
        <div v-if="store.activeTab === 'live'" class="flex items-center gap-1">
          <button
            v-if="isSyncing"
            type="button"
            :disabled="store.status === 'stopping'"
            class="flex min-h-11 items-center gap-1 border-l-2 border-white/20 px-2 text-[10px] text-white/60 disabled:opacity-30 focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-300"
            @click="store.stopForProfileSwitch()"
          >
            <Square :size="12" aria-hidden="true" />
            {{ store.status === "stopping" ? "停止中" : "停止同步" }}
          </button>
          <button
            v-else-if="canRetry"
            type="button"
            :disabled="store.retryInFlight"
            class="flex min-h-11 items-center gap-1 border-l-2 border-blue-400 px-2 text-[10px] text-blue-300 disabled:opacity-30 focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-300"
            @click="store.retrySync"
          >
            <RotateCcw
              :size="12"
              :class="{ 'animate-spin': store.retryInFlight }"
              aria-hidden="true"
            />{{ retryLabel }}
          </button>
          <button
            v-if="store.logs.length > 0"
            type="button"
            class="flex min-h-11 items-center gap-1 rounded px-2 text-[10px] text-white/50 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-300"
            @click="store.copyDiagnostics"
          >
            <Copy :size="12" aria-hidden="true" />复制诊断
          </button>
        </div>
      </footer>
    </div>
  </SlidePage>
</template>
