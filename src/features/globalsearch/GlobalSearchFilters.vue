<script setup lang="ts">
import { CalendarDays, Filter } from "lucide-vue-next";
import type { SearchFilterDraft } from "./types";

const props = defineProps<{ filters: SearchFilterDraft }>();
const emit = defineEmits<{
  (event: "patch", patch: Partial<SearchFilterDraft>): void;
}>();

function inputValue(event: Event): string {
  return (event.target as HTMLInputElement).value;
}

function selectValue(event: Event): string {
  return (event.target as HTMLSelectElement).value;
}

function formatDate(timestamp: number | null): string {
  if (timestamp === null || !Number.isFinite(timestamp)) return "";
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "";
  const year = String(date.getFullYear()).padStart(4, "0");
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function updateDate(field: "startTime" | "endTime", event: Event): void {
  const value = inputValue(event);
  const date = value ? new Date(`${value}T00:00:00`) : null;
  if (field === "endTime" && date) date.setDate(date.getDate() + 1);
  const time = date ? date.getTime() - (field === "endTime" ? 1 : 0) : null;
  emit("patch", { [field]: time });
}

function clearFilters(): void {
  emit("patch", {
    topicId: "",
    ownerType: "",
    ownerId: "",
    speakerAgentId: "",
    role: "",
    startTime: null,
    endTime: null,
  });
}
</script>

<template>
  <section
    id="global-search-filters"
    class="shrink-0 border-b border-white/5 px-4 py-3"
    aria-label="搜索筛选"
  >
    <div class="mb-3 flex items-center justify-between">
      <div class="flex items-center gap-2 text-xs font-bold">
        <Filter :size="15" aria-hidden="true" />筛选条件
      </div>
      <button
        type="button"
        class="min-h-11 px-2 text-[11px] text-primary-text/55 hover:text-primary-text focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-400/60"
        @click="clearFilters"
      >
        清除筛选
      </button>
    </div>
    <div class="grid grid-cols-2 gap-2">
      <label class="field-label"
        >排序
        <select
          :value="props.filters.sort"
          class="field-control"
          @change="
            emit('patch', {
              sort: selectValue($event) as SearchFilterDraft['sort'],
            })
          "
        >
          <option value="time">最新消息</option>
          <option value="rank">相关度</option>
        </select>
      </label>
      <label class="field-label"
        >所有者类型
        <select
          :value="props.filters.ownerType"
          class="field-control"
          @change="
            emit('patch', {
              ownerType: selectValue($event) as SearchFilterDraft['ownerType'],
            })
          "
        >
          <option value="">全部</option>
          <option value="agent">助手</option>
          <option value="group">群组</option>
        </select>
      </label>
      <label class="field-label"
        >每页条数
        <select
          :value="props.filters.limit"
          class="field-control"
          @change="emit('patch', { limit: Number(selectValue($event)) })"
        >
          <option value="25">25</option>
          <option value="50">50</option>
          <option value="100">100</option>
        </select>
      </label>
      <label class="field-label col-span-2"
        >所有者 ID
        <input
          :value="props.filters.ownerId"
          :disabled="!props.filters.ownerType"
          maxlength="256"
          class="field-control"
          placeholder="先选择助手或群组"
          @input="emit('patch', { ownerId: inputValue($event) })"
        />
      </label>
      <label class="field-label col-span-2"
        >话题 ID
        <input
          :value="props.filters.topicId"
          :disabled="!props.filters.ownerType || !props.filters.ownerId"
          maxlength="256"
          class="field-control"
          placeholder="需要完整所有者身份"
          @input="emit('patch', { topicId: inputValue($event) })"
        />
      </label>
      <label class="field-label"
        >角色
        <input
          :value="props.filters.role"
          maxlength="256"
          class="field-control"
          placeholder="user / assistant"
          @input="emit('patch', { role: inputValue($event) })"
        />
      </label>
      <label class="field-label"
        >发言 Agent ID
        <input
          :value="props.filters.speakerAgentId"
          maxlength="256"
          class="field-control"
          placeholder="可选"
          @input="emit('patch', { speakerAgentId: inputValue($event) })"
        />
      </label>
      <label class="field-label"
        ><span class="inline-flex items-center gap-1"
          ><CalendarDays :size="13" aria-hidden="true" />起始日期</span
        >
        <input
          :value="formatDate(props.filters.startTime)"
          type="date"
          min="1970-01-01"
          class="field-control"
          @change="updateDate('startTime', $event)"
        />
      </label>
      <label class="field-label"
        >结束日期
        <input
          :value="formatDate(props.filters.endTime)"
          type="date"
          min="1970-01-01"
          class="field-control"
          @change="updateDate('endTime', $event)"
        />
      </label>
    </div>
  </section>
</template>

<style scoped>
.field-label {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 5px;
  color: color-mix(in srgb, var(--primary-text) 55%, transparent);
  font-size: 10px;
}

.field-control {
  min-height: 42px;
  min-width: 0;
  border: 1px solid color-mix(in srgb, var(--primary-text) 10%, transparent);
  border-radius: 10px;
  background: color-mix(in srgb, var(--primary-text) 6%, transparent);
  padding: 0 10px;
  color: var(--primary-text);
  font-size: 12px;
  outline: none;
}

.field-control:focus-visible {
  border-color: color-mix(in srgb, #60a5fa 65%, transparent);
}

.field-control:disabled {
  cursor: not-allowed;
  opacity: 0.4;
}

@media (prefers-reduced-motion: reduce) {
  .field-control,
  button {
    transition: none;
  }
}
</style>
