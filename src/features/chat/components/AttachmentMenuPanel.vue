<script setup lang="ts">
import type { AttachmentPickerMode } from "../../../core/stores/attachmentNativePicker";

defineProps<{
  visible: boolean;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  (event: "pick", mode: AttachmentPickerMode): void;
}>();

const items: Array<{
  mode: AttachmentPickerMode;
  icon: string;
  iconClass: string;
  label: string;
}> = [
  {
    mode: "camera",
    icon: "i-heroicons-camera",
    iconClass: "text-blue-500/80 group-hover:text-blue-500",
    label: "拍摄",
  },
  {
    mode: "gallery",
    icon: "i-heroicons-photo",
    iconClass: "text-purple-500/80 group-hover:text-purple-500",
    label: "相册",
  },
  {
    mode: "file",
    icon: "i-heroicons-document-text",
    iconClass: "text-orange-500/80 group-hover:text-orange-500",
    label: "文件",
  },
];
</script>

<template>
  <div
    class="overflow-hidden transition-all duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)]"
    :class="visible ? 'opacity-100' : 'opacity-0 pointer-events-none'"
    :style="{ height: visible ? '112px' : '0px' }"
  >
    <div
      class="w-full h-full border-t border-[var(--border-color)]/20 pt-3 pb-2 flex justify-around items-center transition-all duration-300"
      :class="
        visible
          ? 'translate-y-0 opacity-100 scale-100'
          : 'translate-y-4 opacity-0 scale-95'
      "
    >
      <button
        v-for="item in items"
        :key="item.mode"
        :disabled="disabled"
        @click="emit('pick', item.mode)"
        class="group flex flex-col items-center gap-1.5 active:scale-95 transition-all outline-none disabled:opacity-40"
      >
        <div
          class="min-w-[52px] min-h-[52px] flex items-center justify-center rounded-2xl bg-black/5 dark:bg-white/5 border border-[var(--border-color)]/30 group-hover:border-[var(--highlight-text)]/40 transition-all shadow-inner"
        >
          <div
            :class="[
              item.icon,
              item.iconClass,
              'text-2xl group-hover:scale-105 transition-all',
            ]"
          ></div>
        </div>
        <span
          class="text-[11px] font-semibold text-[var(--primary-text)]/70 group-hover:text-[var(--primary-text)] transition-colors"
        >
          {{ item.label }}
        </span>
      </button>
    </div>
  </div>
</template>
