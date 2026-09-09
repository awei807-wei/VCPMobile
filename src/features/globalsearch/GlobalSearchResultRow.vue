<script setup lang="ts">
import { computed } from "vue";
import { ChevronRight, MessageCircle } from "lucide-vue-next";
import { splitHighlight } from "./safeHighlight";
import type { SearchResult } from "./types";

const props = defineProps<{
  result: SearchResult;
  query: string;
}>();

const emit = defineEmits<{
  (event: "select", result: SearchResult): void;
}>();

const segments = computed(() =>
  splitHighlight(props.result.snippet, props.query),
);
const timestamp = computed(() => {
  if (!Number.isFinite(props.result.timestamp) || props.result.timestamp <= 0)
    return "未知时间";
  return new Intl.DateTimeFormat(undefined, {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(props.result.timestamp);
});

const speaker = computed(
  () =>
    props.result.speakerAgentId ||
    (props.result.role === "user" ? "用户" : props.result.role || "消息"),
);
</script>

<template>
  <button
    type="button"
    class="global-search-result group w-full rounded-xl border border-transparent px-3 py-3 text-left transition-colors hover:border-white/10 hover:bg-white/5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60 active:scale-[0.99]"
    :aria-label="`打开话题 ${result.topicTitle || '未命名话题'}`"
    @click="emit('select', result)"
  >
    <div class="flex items-start gap-2">
      <MessageCircle
        :size="16"
        class="mt-0.5 shrink-0 text-blue-300/75"
        aria-hidden="true"
      />
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <span class="truncate text-[13px] font-bold text-primary-text">{{
            result.topicTitle || "未命名话题"
          }}</span>
          <span class="shrink-0 text-[10px] text-primary-text/40">{{
            timestamp
          }}</span>
        </div>
        <div
          class="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[10px] text-primary-text/45"
        >
          <span>{{ result.ownerType === "agent" ? "助手" : "群组" }}</span>
          <span class="max-w-[10rem] truncate">{{ speaker }}</span>
          <span v-if="result.rank !== null"
            >相关度 {{ result.rank.toFixed(2) }}</span
          >
        </div>
        <p
          class="mt-2 break-words text-[12px] leading-relaxed text-primary-text/75"
        >
          <template
            v-for="(segment, index) in segments"
            :key="`${index}-${segment.text}`"
          >
            <mark
              v-if="segment.highlighted"
              class="rounded bg-blue-400/20 px-0.5 text-blue-100"
              >{{ segment.text }}</mark
            >
            <span v-else>{{ segment.text }}</span>
          </template>
        </p>
      </div>
      <ChevronRight
        :size="16"
        class="mt-0.5 shrink-0 text-primary-text/25 transition-transform group-hover:translate-x-0.5"
        aria-hidden="true"
      />
    </div>
  </button>
</template>

<style scoped>
.global-search-result {
  min-height: 88px;
}

@media (prefers-reduced-motion: reduce) {
  .global-search-result,
  .global-search-result :deep(svg) {
    transition: none;
  }

  .global-search-result:active {
    transform: none;
  }
}
</style>
