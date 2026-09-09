<script setup lang="ts">
import { computed } from "vue";
import {
  AlertTriangle,
  CheckCircle2,
  Database,
  RefreshCw,
  XCircle,
} from "lucide-vue-next";
import type { FtsIndexStatus, IndexState } from "./types";

const props = defineProps<
  {
    state: IndexState;
    status: FtsIndexStatus | null;
  } & {
    rebuilding?: boolean;
  }
>();

const emit = defineEmits<{
  (event: "refresh"): void;
  (event: "rebuild"): void;
}>();

const title = computed(() => {
  switch (props.state) {
    case "checking":
      return "正在检查本地索引";
    case "healthy":
      return "本地索引正常";
    case "missing":
      return "本地索引不可用";
    case "inconsistent":
      return "本地索引需要修复";
    case "rebuilding":
      return "本地索引重建中";
    case "failed":
      return "本地索引重建失败";
    case "unavailable":
      return "暂时无法检查索引";
    default:
      return "等待检查本地索引";
  }
});

const detail = computed(() => {
  if (props.state === "checking") return "首屏只检查状态，不会自动重建。";
  if (props.state === "missing") return "索引结构或分词器不可用，可手动重建。";
  if (props.state === "inconsistent")
    return props.status?.decodeErrorCount
      ? "部分消息正文无法解码；重建会保持原索引并报告失败。"
      : "检测到缺失、孤立、重复或过期条目。";
  if (props.state === "rebuilding")
    return "重建期间仍保留旧索引，完成后再检查。";
  if (props.state === "failed") return "重建未完成，原有索引保持不变。";
  if (props.state === "unavailable") return "数据库尚未就绪，请稍后重试。";
  return props.state === "healthy"
    ? "搜索结果来自本地数据库。"
    : "可以开始输入消息内容。";
});

const icon = computed(() => {
  if (props.state === "healthy") return CheckCircle2;
  if (
    props.state === "missing" ||
    props.state === "failed" ||
    props.state === "unavailable"
  )
    return XCircle;
  if (props.state === "inconsistent") return AlertTriangle;
  return Database;
});

const hasRepairIssue = computed(() =>
  ["missing", "inconsistent", "failed"].includes(props.state),
);
</script>

<template>
  <section class="global-search-index" aria-live="polite">
    <div class="flex items-start gap-3">
      <component
        :is="icon"
        :size="18"
        class="mt-0.5 shrink-0 opacity-80"
        aria-hidden="true"
      />
      <div class="min-w-0 flex-1">
        <p class="text-xs font-bold text-primary-text">{{ title }}</p>
        <p class="mt-1 text-[11px] leading-relaxed text-primary-text/55">
          {{ detail }}
        </p>
        <p v-if="status" class="mt-1 text-[10px] text-primary-text/45">
          {{ status.indexedCount }} / {{ status.liveCount }} 条已建立索引
          <span
            v-if="
              status.missingCount ||
              status.orphanCount ||
              status.duplicateCount ||
              status.staleCount ||
              status.decodeErrorCount
            "
          >
            · 待处理
            {{
              status.missingCount +
              status.orphanCount +
              status.duplicateCount +
              status.staleCount +
              status.decodeErrorCount
            }}
            条
          </span>
        </p>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        <button
          type="button"
          class="min-h-11 min-w-11 rounded-xl text-primary-text/60 transition-colors hover:bg-white/5 hover:text-primary-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60 disabled:opacity-30"
          aria-label="重新检查索引"
          :disabled="state === 'checking' || rebuilding"
          @click="emit('refresh')"
        >
          <RefreshCw
            :size="16"
            class="mx-auto"
            :class="{ 'animate-spin': state === 'checking' }"
          />
        </button>
        <button
          v-if="hasRepairIssue"
          type="button"
          class="min-h-11 rounded-xl bg-blue-500/15 px-3 text-[11px] font-bold text-blue-300 transition-colors hover:bg-blue-500/25 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60 disabled:opacity-40"
          :disabled="rebuilding"
          @click="emit('rebuild')"
        >
          {{ rebuilding ? "重建中" : "手动重建" }}
        </button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.global-search-index {
  border-bottom: 1px solid
    color-mix(in srgb, var(--primary-text) 8%, transparent);
  padding: 10px 16px;
  background: color-mix(in srgb, var(--secondary-bg) 65%, transparent);
}

@media (prefers-reduced-motion: reduce) {
  .animate-spin {
    animation: none;
  }
}
</style>
