<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { BellRing, ChevronsDown, ChevronsUp } from "lucide-vue-next";
import { useNotificationStore } from "../../core/stores/notification";
import ToastItem from "./ToastItem.vue";

const store = useNotificationStore();
const COLLAPSED_TOAST_LIMIT = 2;
const isExpanded = ref(false);

const hasOverflow = computed(
  () => store.activeToasts.length > COLLAPSED_TOAST_LIMIT,
);
const hiddenToastCount = computed(() =>
  Math.max(0, store.activeToasts.length - COLLAPSED_TOAST_LIMIT),
);
const visibleToasts = computed(() => {
  if (isExpanded.value || !hasOverflow.value) return store.activeToasts;
  return store.activeToasts.slice(-COLLAPSED_TOAST_LIMIT);
});
const toggleLabel = computed(() =>
  isExpanded.value
    ? `收起通知，仅显示最近 ${COLLAPSED_TOAST_LIMIT} 条`
    : `展开全部 ${store.activeToasts.length} 条通知`,
);

watch(
  () => store.activeToasts.length,
  (count) => {
    if (count <= COLLAPSED_TOAST_LIMIT) isExpanded.value = false;
  },
);
</script>

<template>
  <div
    class="vcp-toast-stack fixed left-0 right-0 z-toast pointer-events-none px-4 flex flex-col items-center gap-2.5"
  >
    <section
      v-if="store.activeToasts.length > 0"
      class="vcp-toast-panel"
      :class="{ 'is-expanded': isExpanded }"
      aria-label="当前通知"
    >
      <button
        v-if="hasOverflow"
        type="button"
        class="vcp-toast-toggle"
        :aria-expanded="isExpanded"
        aria-controls="vcp-active-toast-list"
        :aria-label="toggleLabel"
        data-testid="toast-collapse-toggle"
        @click="isExpanded = !isExpanded"
      >
        <BellRing :size="14" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate text-left">
          {{
            isExpanded
              ? `共 ${store.activeToasts.length} 条通知`
              : `已收起 ${hiddenToastCount} 条通知`
          }}
        </span>
        <span class="font-black">{{ isExpanded ? "收起" : "展开" }}</span>
        <ChevronsUp v-if="isExpanded" :size="14" aria-hidden="true" />
        <ChevronsDown v-else :size="14" aria-hidden="true" />
      </button>

      <TransitionGroup
        id="vcp-active-toast-list"
        name="toast"
        tag="div"
        class="vcp-toast-list"
        aria-live="polite"
      >
        <ToastItem
          v-for="toast in visibleToasts"
          :key="toast.id"
          :toast="toast"
          :compact="hasOverflow && !isExpanded"
        />
      </TransitionGroup>
    </section>
  </div>
</template>

<style scoped>
.vcp-toast-stack {
  top: calc(var(--vcp-safe-top, env(safe-area-inset-top, 0px)) + 16px);
}

.vcp-toast-panel {
  width: 100%;
  max-width: 320px;
  max-height: calc(
    100dvh - max(var(--vcp-safe-top, env(safe-area-inset-top, 0px)), 24px) -
      36px
  );
  pointer-events: auto;
  scrollbar-width: none;
}

.vcp-toast-panel::-webkit-scrollbar {
  display: none;
}

.vcp-toast-panel.is-expanded {
  overflow-y: auto;
  overscroll-behavior: contain;
  -webkit-overflow-scrolling: touch;
  padding-bottom: 2px;
}

.vcp-toast-list {
  display: flex;
  flex-direction: column;
  gap: 0.625rem;
}

.vcp-toast-toggle {
  position: sticky;
  top: 0;
  z-index: 1;
  display: flex;
  width: 100%;
  min-height: 44px;
  align-items: center;
  gap: 0.5rem;
  margin-bottom: 0.625rem;
  padding: 0.625rem 0.875rem;
  border: 1px solid rgb(0 0 0 / 8%);
  border-radius: 0.75rem;
  background: rgb(255 255 255 / 94%);
  color: var(--primary-text);
  box-shadow: 0 8px 24px rgb(0 0 0 / 12%);
  backdrop-filter: blur(12px);
  font-size: 0.6875rem;
  transition:
    transform 160ms ease,
    border-color 160ms ease;
}

:global(.dark) .vcp-toast-toggle {
  border-color: rgb(255 255 255 / 12%);
  background: rgb(24 24 27 / 94%);
}

.vcp-toast-toggle:active {
  transform: scale(0.98);
}

.vcp-toast-toggle:focus-visible {
  outline: 2px solid var(--highlight-text);
  outline-offset: 2px;
}

@media (pointer: coarse) {
  .vcp-toast-stack {
    /* Android edge-to-edge WebView often reports safe-area as 0, so keep toasts below the status bar. */
    top: calc(
      max(var(--vcp-safe-top, env(safe-area-inset-top, 0px)), 24px) + 12px
    );
  }
}

.toast-enter-active {
  transition: all 0.5s cubic-bezier(0.18, 0.89, 0.32, 1.28);
}

.toast-leave-active {
  transition: all 0.4s ease-in;
}

.toast-enter-from {
  opacity: 0;
  transform: translateY(-40px) scale(0.8);
}

.toast-leave-to {
  opacity: 0;
  transform: translateY(-20px) scale(0.9);
}

.toast-move {
  transition: transform 0.4s ease;
}

@media (prefers-reduced-motion: reduce) {
  .vcp-toast-toggle,
  .toast-enter-active,
  .toast-leave-active,
  .toast-move {
    transition: none;
  }
}
</style>
