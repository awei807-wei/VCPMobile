<script setup lang="ts">
import { useTarvenStore } from "../../../core/stores/tarvenStore";

defineProps<{
  visible: boolean;
  disabled?: boolean;
  hasContent: boolean;
  isGenerating: boolean;
  sendDisabled: boolean;
}>();

const emit = defineEmits<{
  (event: "toggle", visible: boolean): void;
  (event: "action"): void;
}>();

const tarvenStore = useTarvenStore();

const openTarvenSelector = () => {
  if (tarvenStore.rules.some((rule) => rule.isEnabled)) {
    if (navigator.vibrate) navigator.vibrate(50);
    tarvenStore.isSelectorOpen = true;
  }
};
</script>

<template>
  <div class="flex items-center shrink-0 mb-0.5 relative gap-1.5">
    <button
      v-longpress="openTarvenSelector"
      :disabled="disabled"
      @click="emit('toggle', !visible)"
      class="min-w-[44px] min-h-[44px] flex items-center justify-center rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-80 hover:opacity-100 active:scale-90 transition-all relative disabled:opacity-40"
      aria-label="附件与规则"
    >
      <div
        class="i-heroicons-plus-circle text-2xl transition-transform duration-300 ease-out"
        :class="{ 'rotate-45': visible }"
      ></div>
      <div
        v-if="tarvenStore.rules.some((rule) => rule.isEnabled)"
        class="absolute top-1.5 right-1.5 w-2 h-2 bg-emerald-500 rounded-full border-2 border-[var(--secondary-bg)] shadow-[0_0_8px_rgba(16,185,129,0.5)]"
      ></div>
    </button>

    <Transition name="pop-slide">
      <button
        v-if="hasContent || isGenerating"
        :disabled="!isGenerating && sendDisabled"
        @click="emit('action')"
        class="min-w-[44px] min-h-[44px] flex items-center justify-center rounded-full shadow-sm active:scale-95 transition-all bg-blue-500 text-white disabled:opacity-40"
        :class="{ 'bg-red-500': isGenerating }"
        aria-label="发送消息"
      >
        <div
          v-if="isGenerating"
          class="i-heroicons-stop-16-solid text-lg"
        ></div>
        <div
          v-else
          class="i-heroicons-paper-airplane text-[15px] -rotate-45 translate-x-0.2 -translate-y-0.2"
        ></div>
      </button>
    </Transition>
  </div>
</template>

<style scoped>
.pop-slide-enter-active,
.pop-slide-leave-active {
  transition: all 0.3s cubic-bezier(0.34, 1.56, 0.64, 1);
  overflow: hidden;
  white-space: nowrap;
}

.pop-slide-enter-from {
  opacity: 0;
  transform: scale(0.4) translateX(20px);
  width: 0;
}

.pop-slide-leave-to {
  opacity: 0;
  transform: scale(0.4) translateX(10px);
  width: 0;
  margin-left: -6px;
}
</style>
