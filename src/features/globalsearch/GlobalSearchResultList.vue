<script setup lang="ts">
import { LoaderCircle, SearchX, TriangleAlert } from "lucide-vue-next";
import GlobalSearchResultRow from "./GlobalSearchResultRow.vue";
import type { SearchResult, SearchStatus } from "./types";

defineProps<{
  results: SearchResult[];
  query: string;
  status: SearchStatus;
  hasMore: boolean;
}>();

const emit = defineEmits<{
  (event: "select", result: SearchResult): void;
  (event: "load-more"): void;
  (event: "retry"): void;
}>();
</script>

<template>
  <div
    class="global-search-results flex-1 overflow-y-auto px-3 pb-[calc(var(--vcp-safe-bottom,16px)+16px)] pt-2"
    aria-live="polite"
  >
    <div
      v-if="status === 'loading' && results.length === 0"
      class="flex min-h-48 flex-col items-center justify-center gap-3 text-primary-text/45"
    >
      <LoaderCircle
        :size="22"
        class="animate-spin text-blue-300"
        aria-hidden="true"
      />
      <span class="text-xs">正在搜索本地消息…</span>
    </div>

    <div
      v-else-if="status === 'invalid'"
      class="flex min-h-48 flex-col items-center justify-center gap-2 px-8 text-center text-primary-text/55"
    >
      <TriangleAlert :size="22" class="text-amber-300/80" aria-hidden="true" />
      <span class="text-xs">搜索词不符合限制，请修改后重试。</span>
    </div>

    <div
      v-else-if="status === 'error'"
      class="flex min-h-48 flex-col items-center justify-center gap-3 px-8 text-center text-primary-text/55"
    >
      <TriangleAlert :size="22" class="text-red-300/80" aria-hidden="true" />
      <span class="text-xs">搜索暂时失败，已有结果不会被清除。</span>
      <button
        type="button"
        class="min-h-11 rounded-xl bg-white/10 px-4 text-xs font-bold text-primary-text hover:bg-white/15 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60"
        @click="emit('retry')"
      >
        重试
      </button>
    </div>

    <div
      v-else-if="status === 'empty'"
      class="flex min-h-48 flex-col items-center justify-center gap-2 text-primary-text/45"
    >
      <SearchX :size="24" aria-hidden="true" />
      <span class="text-xs">没有找到匹配消息</span>
    </div>

    <div
      v-else-if="status === 'idle'"
      class="flex min-h-48 flex-col items-center justify-center gap-2 px-8 text-center text-primary-text/40"
    >
      <SearchX :size="24" aria-hidden="true" />
      <span class="text-xs">输入至少 1 个字符，搜索所有话题消息</span>
    </div>

    <template v-else>
      <div class="space-y-1">
        <GlobalSearchResultRow
          v-for="result in results"
          :key="`${result.ownerType}:${result.ownerId}:${result.topicId}:${result.msgId}`"
          :result="result"
          :query="query"
          @select="emit('select', $event)"
        />
      </div>
      <button
        v-if="hasMore"
        type="button"
        class="mt-3 min-h-11 w-full rounded-xl border border-white/10 bg-white/5 text-xs font-bold text-primary-text/70 hover:bg-white/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60 disabled:opacity-40"
        :disabled="status === 'loading'"
        @click="emit('load-more')"
      >
        {{ status === "loading" ? "正在加载…" : "加载更多" }}
      </button>
      <p
        v-if="status === 'loading'"
        class="py-3 text-center text-[10px] text-primary-text/35"
      >
        正在更新结果…
      </p>
    </template>
  </div>
</template>

<style scoped>
.global-search-results {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.global-search-results::-webkit-scrollbar {
  display: none;
}

@media (prefers-reduced-motion: reduce) {
  .animate-spin {
    animation: none;
  }
}
</style>
