<script setup lang="ts">
import { computed } from "vue";
import { useTarvenStore } from "../../../core/stores/tarvenStore";
import { useThemeStore } from "../../../core/stores/theme";

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
const themeStore = useThemeStore();
const isWechatLayout = computed(() => themeStore.isWechatLayout);

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
      <!-- 微信布局：空闲且无内容时整颗按钮不渲染，只留左侧「+」 -->
      <button
        v-if="!isWechatLayout || isGenerating || hasContent"
        :disabled="!isGenerating && (!hasContent || sendDisabled)"
        @click="emit('action')"
        class="flex items-center justify-center active:scale-95 transition-all disabled:opacity-40"
        :class="
          isWechatLayout
            ? [
                'h-[34px] rounded-[4px] whitespace-nowrap',
                isGenerating
                  ? 'px-2.5 bg-black/10 dark:bg-white/10 text-[var(--primary-text)] text-[12px]'
                  : 'px-3 bg-[#07C160] hover:bg-[#06AD56] text-white text-[13px] font-medium',
              ]
            : [
                'min-w-[44px] min-h-[44px] rounded-full shadow-sm text-white',
                isGenerating ? 'bg-red-500' : 'bg-blue-500',
              ]
        "
        :aria-label="isGenerating ? '停止生成' : '发送消息'"
      >
        <template v-if="isWechatLayout">
          <span class="leading-none">{{ isGenerating ? "停止" : "发送" }}</span>
        </template>
        <template v-else>
          <div
            v-if="isGenerating"
            class="i-heroicons-stop-16-solid text-lg"
          ></div>
          <div
            v-else
            class="i-heroicons-paper-airplane text-[15px] -rotate-45 translate-x-0.2 -translate-y-0.2"
          ></div>
        </template>
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
