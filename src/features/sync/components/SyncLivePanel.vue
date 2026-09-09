<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import {
  AlertTriangle,
  CircleCheck,
  Play,
} from "lucide-vue-next";
import SettingsSwitch from "../../../components/settings/SettingsSwitch.vue";
import { useSettingsStore } from "../../../core/stores/settings";
import { useOverlayStore } from "../../../core/stores/overlay";
import { useSyncSessionStore } from "../../../core/stores/syncSession";

const store = useSyncSessionStore();
const settingsStore = useSettingsStore();
const overlayStore = useOverlayStore();
const logContainer = ref<HTMLElement | null>(null);

const visibleLogs = computed(() =>
  store.logs.slice(Math.max(0, store.logs.length - 100)),
);
const progressPercent = computed(() => {
  if (
    store.status === "completed" ||
    store.status === "completed_with_warnings"
  )
    return 100;
  if (store.progressData.total <= 0) return 0;
  return Math.min(
    100,
    Math.round((store.progressData.completed / store.progressData.total) * 100),
  );
});
const phaseLabel = computed(
  () =>
    ({
      initialization: "初始化",
      owner_metadata: "所有者元数据",
      topic_metadata: "会话主题同步",
      topic_validation: "会话校验",
      messages: "历史消息同步",
      finalize: "数据收尾",
    })[store.progressData.phase] ?? "同步处理",
);
const isProgressIndeterminate = computed(
  () => store.isActive && store.progressData.total <= 0,
);
const prerenderEnabled = computed(
  () => settingsStore.settings?.syncPrerenderEnabled ?? false,
);
const errorStageLabels: Record<string, string> = {
  preflight: "设备预检",
  startup: "同步启动",
  connect: "建立连接",
  handshake: "版本握手",
  owner_metadata: "所有者元数据",
  topic_metadata: "话题元数据",
  topic_validation: "话题校验",
  messages: "消息同步",
  finalize: "同步收尾",
  shutdown: "同步退出",
  history: "历史续传",
};
const errorStageLabel = computed(
  () => errorStageLabels[store.terminalError?.stage ?? ""] ?? "同步处理",
);
const errorOriginLabels: Record<string, string> = {
  mobile_ui: "手机界面",
  mobile_native: "手机系统",
  mobile_sync: "手机同步核心",
  desktop_plugin: "电脑同步插件",
  desktop_cds: "电脑数据服务",
};
const errorOriginLabel = computed(
  () => errorOriginLabels[store.terminalError?.origin ?? ""] ?? "同步组件",
);

const logColor = (level: string) =>
  ({
    success: "text-green-400",
    error: "text-red-400",
    warning: "text-yellow-400",
  })[level] ?? "text-blue-300";

watch(
  () => store.logs.length,
  () => {
    nextTick(() => {
      if (logContainer.value)
        logContainer.value.scrollTop = logContainer.value.scrollHeight;
    });
  },
);

const handlePrerenderToggle = async (value: boolean) => {
  if (value) {
    const confirmed = await overlayStore.showConfirm({
      title: "开启预渲染",
      message:
        "启用后将在同步时进行预渲染计算，可能导致同步耗时增加，首次同步建议关闭。确认启用？",
    });
    if (!confirmed) return;
  }
  await settingsStore.updateSettings({ syncPrerenderEnabled: value });
};
</script>

<template>
  <div
    id="sync-live-panel"
    role="tabpanel"
    aria-labelledby="sync-live-tab"
    class="h-full flex flex-col overflow-hidden"
  >
    <div
      v-if="store.status === 'idle'"
      class="flex-1 flex flex-col items-center justify-center px-8"
    >
      <div
        class="w-16 h-16 rounded-full bg-white/5 flex items-center justify-center mb-6"
      >
        <Play :size="28" class="text-blue-400 ml-1" aria-hidden="true" />
      </div>
      <div class="text-sm font-bold tracking-wider mb-2">全量神经同步</div>
      <div class="text-[11px] text-white/30 text-center mb-8 leading-relaxed">
        双向比对并合并智能体、群组、话题、头像与历史消息<br />
        较新的修改和删除会同步；附件仅同步信息，不传输文件
      </div>
      <button
        type="button"
        class="min-h-11 px-8 py-3 rounded-lg bg-blue-500/20 text-blue-300 text-xs font-bold tracking-widest uppercase transition-colors active:bg-blue-500/30 focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-300"
        @click="store.startSync()"
      >
        开始同步
      </button>
      <button
        type="button"
        class="mt-3 min-h-11 px-3 text-[10px] text-white/35 transition-colors hover:text-white/70 focus-visible:outline focus-visible:outline-2 focus-visible:outline-blue-300"
        @click="store.switchTab('history')"
      >
        查看历史日志
      </button>
      <div class="w-full max-w-xs mt-6 border-t border-white/5 pt-4">
        <div
          class="text-[9px] font-bold uppercase tracking-widest text-white/20 mb-3"
        >
          高级设置
        </div>
        <div class="flex items-center justify-between gap-4">
          <div class="flex flex-col text-left">
            <span class="text-[12px] font-semibold text-white/70"
              >预渲染同步</span
            >
            <span class="text-[9px] text-white/25 mt-0.5"
              >同步时预编译渲染缓存，首次同步建议关闭</span
            >
          </div>
          <SettingsSwitch
            :model-value="prerenderEnabled"
            @update:model-value="handlePrerenderToggle"
          />
        </div>
      </div>
    </div>

    <template v-else>
      <div class="shrink-0 px-4 pt-3 pb-2">
        <div class="h-1 bg-white/10 rounded-full overflow-hidden">
          <div
            v-if="isProgressIndeterminate"
            class="sync-progress-indeterminate h-full rounded-full bg-blue-400"
          ></div>
          <div
            v-else
            class="h-full transition-all duration-500 rounded-full"
            :class="
              store.status === 'error'
                ? 'bg-red-500'
                : store.status === 'completed_with_warnings'
                  ? 'bg-yellow-500'
                  : store.status === 'completed'
                    ? 'bg-green-500'
                    : 'bg-blue-500'
            "
            :style="{ width: `${progressPercent}%` }"
          ></div>
        </div>
        <div class="flex justify-between text-[10px] mt-1 text-white/50">
          <span>{{ phaseLabel }}</span>
          <span v-if="store.progressData.total > 0"
            >{{ store.progressData.completed }}/{{ store.progressData.total }} ·
            {{ progressPercent }}%</span
          >
        </div>
      </div>

      <div
        v-if="
          store.summary.totalTopics > 0 ||
          ['completed', 'completed_with_warnings', 'error'].includes(
            store.status,
          )
        "
        class="grid grid-cols-4 mx-4 border-y border-white/8 py-2 font-mono text-center"
      >
        <div>
          <div class="text-[9px] text-white/30">成功</div>
          <div class="text-xs text-green-400">
            {{ store.summary.successfulTopics }}
          </div>
        </div>
        <div>
          <div class="text-[9px] text-white/30">总数</div>
          <div class="text-xs text-white/70">
            {{ store.summary.totalTopics }}
          </div>
        </div>
        <div>
          <div class="text-[9px] text-white/30">失败</div>
          <div
            class="text-xs"
            :class="
              store.summary.failedTopics > 0 ? 'text-red-400' : 'text-white/50'
            "
          >
            {{ store.summary.failedTopics }}
          </div>
        </div>
        <div>
          <div class="text-[9px] text-white/30">旧附件</div>
          <div
            class="text-xs"
            :class="
              store.summary.legacyAttachmentWarnings > 0
                ? 'text-yellow-400'
                : 'text-white/50'
            "
          >
            {{ store.summary.legacyAttachmentWarnings }}
          </div>
        </div>
      </div>

      <div
        v-if="store.terminalError"
        class="mx-4 mt-3 border-l-2 border-red-500 bg-red-500/6 px-3 py-2 text-left"
      >
        <div
          class="flex items-start gap-2 text-[11px] font-semibold leading-relaxed text-red-300 break-words"
        >
          <AlertTriangle
            :size="15"
            class="mt-0.5 shrink-0"
            aria-hidden="true"
          />{{ store.terminalError.message }}
        </div>
        <div class="mt-1 text-[10px] leading-relaxed text-white/55 break-words">
          {{ store.terminalError.guidance }}
        </div>
        <div
          class="mt-2 font-mono text-[9px] leading-relaxed text-white/35 break-all"
        >
          {{ errorStageLabel }} · {{ errorOriginLabel }} ·
          {{ store.terminalError.code }}
        </div>
        <div class="mt-2 text-[9px] text-white/30">
          {{
            store.terminalError.logFile
              ? "详细记录已保存至历史日志。"
              : "本次未生成诊断日志。"
          }}
        </div>
      </div>
      <div
        v-else-if="store.status === 'completed_with_warnings'"
        class="mx-4 mt-3 border-l-2 border-yellow-500 bg-yellow-500/6 px-3 py-2 text-left"
      >
        <div
          class="flex items-start gap-2 text-[11px] font-semibold leading-relaxed text-yellow-300"
        >
          <CircleCheck
            :size="15"
            class="mt-0.5 shrink-0"
            aria-hidden="true"
          />消息已同步，{{ store.summary.legacyAttachmentWarnings }}
          项旧附件信息无法安全识别，已跳过
        </div>
        <div class="mt-1 text-[10px] leading-relaxed text-white/55">
          请在电脑端重新发送这些附件后，再重新同步。
        </div>
      </div>

      <div class="flex-1 px-4 mt-3 overflow-hidden flex flex-col min-h-0">
        <div
          ref="logContainer"
          class="bg-black/40 rounded-lg p-3 font-mono text-[10px] leading-relaxed flex-1 overflow-y-auto no-rubber-band min-h-0"
        >
          <div
            v-if="store.logs.length === 0"
            class="flex h-full items-center justify-center text-white/25 italic"
          >
            {{
              store.status === "stopped"
                ? "本次同步已停止，暂无日志"
                : "等待同步事件..."
            }}
          </div>
          <template v-else>
            <div
              v-for="log in visibleLogs"
              :key="log.id"
              class="break-words mb-0.5"
              :class="logColor(log.level)"
            >
              [{{ log.time }}] {{ log.message }}
            </div>
            <div
              v-if="store.logs.length > 100"
              class="text-white/20 text-center py-1"
            >
              ... {{ store.logs.length - 100 }} 条更早的日志已折叠
            </div>
          </template>
        </div>
      </div>

    </template>
  </div>
</template>

<style scoped>
.sync-progress-indeterminate {
  width: 36%;
  animation: sync-progress-slide 1.2s ease-in-out infinite;
}

@keyframes sync-progress-slide {
  from {
    transform: translateX(-110%);
  }
  to {
    transform: translateX(290%);
  }
}

@media (prefers-reduced-motion: reduce) {
  .sync-progress-indeterminate {
    animation: none;
    width: 50%;
  }
}
</style>
