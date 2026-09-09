<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { ArrowLeft, Search, SlidersHorizontal, X } from "lucide-vue-next";
import SlidePage from "../../components/ui/SlidePage.vue";
import { useLayoutStore } from "../../core/stores/layout";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import GlobalSearchFilters from "./GlobalSearchFilters.vue";
import GlobalSearchIndexStatus from "./GlobalSearchIndexStatus.vue";
import GlobalSearchResultList from "./GlobalSearchResultList.vue";
import { navigateToSearchResult } from "./navigation";
import { useGlobalSearchStore } from "./useGlobalSearchStore";
import type { SearchFilterDraft, SearchResult } from "./types";

const props = withDefaults(
  defineProps<{ isOpen: boolean; zIndex?: number }>(),
  { zIndex: 40 },
);
const emit = defineEmits<{ (event: "close"): void }>();

const layoutStore = useLayoutStore();
const sessionStore = useChatSessionStore();
const searchStore = useGlobalSearchStore();
const router = useRouter();
const filtersExpanded = ref(false);

const activeFilterCount = computed(() => {
  const filters = searchStore.filters;
  return [
    filters.topicId,
    filters.ownerType,
    filters.ownerId,
    filters.speakerAgentId,
    filters.role,
    filters.startTime,
    filters.endTime,
  ].filter(Boolean).length;
});

watch(
  () => props.isOpen,
  (isOpen) => {
    if (isOpen) searchStore.open();
    else searchStore.close();
  },
  { immediate: true },
);

const close = () => emit("close");

const handleQueryInput = (event: Event) => {
  searchStore.setQuery((event.target as HTMLInputElement).value);
};

const updateFilter = (patch: Partial<SearchFilterDraft>) =>
  searchStore.setFilter(patch);

const handleSelect = async (result: SearchResult) => {
  const outcome = await navigateToSearchResult(result, {
    router,
    sessionStore,
    layoutStore,
    searchStore,
  });
  if (outcome === "pending") close();
};

const handleRebuild = () => {
  void searchStore.rebuildIndex();
};
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <main
      class="global-search-view flex h-full w-full flex-col bg-[var(--primary-bg)] text-primary-text pointer-events-auto"
    >
      <header
        class="shrink-0 border-b border-white/5 bg-[var(--secondary-bg)]/95 px-4 pb-3 pt-[calc(var(--vcp-safe-top,24px)+8px)]"
      >
        <div class="flex items-center gap-2">
          <button
            type="button"
            class="min-h-11 min-w-11 rounded-xl text-primary-text/70 hover:bg-white/5 hover:text-primary-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60"
            aria-label="关闭全局搜索"
            @click="close"
          >
            <ArrowLeft :size="19" class="mx-auto" />
          </button>
          <div class="min-w-0 flex-1">
            <h1 class="text-base font-black tracking-tight">全局搜索</h1>
            <p class="text-[10px] text-primary-text/45">
              搜索所有话题中的本地消息
            </p>
          </div>
          <button
            type="button"
            class="min-h-11 min-w-11 rounded-xl text-primary-text/65 hover:bg-white/5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60"
            :class="{ 'text-blue-300': filtersExpanded || activeFilterCount }"
            aria-label="展开搜索筛选"
            aria-controls="global-search-filters"
            :aria-expanded="filtersExpanded"
            @click="filtersExpanded = !filtersExpanded"
          >
            <SlidersHorizontal :size="18" class="mx-auto" />
          </button>
        </div>

        <div class="relative mt-3">
          <Search
            :size="17"
            class="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-primary-text/45"
            aria-hidden="true"
          />
          <input
            :value="searchStore.filters.query"
            type="search"
            inputmode="search"
            autocomplete="off"
            maxlength="128"
            placeholder="输入消息内容，1 个字符即可搜索"
            aria-label="搜索消息"
            class="min-h-12 w-full rounded-xl border border-white/10 bg-black/10 px-10 pr-11 text-sm text-primary-text outline-none transition-colors placeholder:text-primary-text/35 focus:border-blue-400/60"
            @input="handleQueryInput"
          />
          <button
            v-if="searchStore.filters.query"
            type="button"
            class="absolute right-1 top-1/2 min-h-10 min-w-10 -translate-y-1/2 rounded-lg text-primary-text/50 hover:bg-white/5 hover:text-primary-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60"
            aria-label="清除搜索词"
            @click="searchStore.setQuery('')"
          >
            <X :size="16" class="mx-auto" />
          </button>
        </div>
        <p
          v-if="searchStore.validationMessage"
          class="mt-2 text-[11px] text-amber-300"
        >
          {{ searchStore.validationMessage }}
        </p>
      </header>

      <GlobalSearchIndexStatus
        :state="searchStore.indexState"
        :status="searchStore.indexStatus"
        :rebuilding="searchStore.indexState === 'rebuilding'"
        @refresh="searchStore.refreshIndexStatus()"
        @rebuild="handleRebuild"
      />

      <GlobalSearchFilters
        v-if="filtersExpanded"
        :filters="searchStore.filters"
        @patch="updateFilter"
      />

      <p
        v-if="
          searchStore.targetStatus === 'error' ||
          searchStore.targetStatus === 'invalid'
        "
        class="border-b border-red-300/10 bg-red-400/5 px-4 py-2 text-[11px] text-red-200/80"
        role="status"
      >
        目标消息已失效或暂时无法定位，请重新打开搜索结果后重试。
      </p>
      <GlobalSearchResultList
        :results="searchStore.results"
        :query="searchStore.filters.query"
        :status="searchStore.searchStatus"
        :has-more="searchStore.hasMore"
        @select="handleSelect"
        @load-more="searchStore.loadMore()"
        @retry="searchStore.retry()"
      />
    </main>
  </SlidePage>
</template>

<style scoped>
@media (prefers-reduced-motion: reduce) {
  .global-search-view button,
  .global-search-view input {
    transition: none;
  }
}
</style>
