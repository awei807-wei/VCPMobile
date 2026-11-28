<script setup lang="ts">
import { computed } from "vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import { useThemeStore } from "../../core/stores/theme";

type OwnerType = "agent" | "group";

const themeStore = useThemeStore();
const isWechatLayout = computed(() => themeStore.isWechatLayout);

defineProps<{
  ownerType: OwnerType;
  ownerId: string;
  name: string;
  subtitle: string;
  selected: boolean;
  swipeX: number;
  swipeActive: boolean;
  isDragging: boolean;
  showUnread?: boolean;
}>();

const emit = defineEmits<{
  (event: "select"): void;
  (event: "settings"): void;
  (event: "touch-start", payload: TouchEvent): void;
  (event: "touch-move", payload: TouchEvent): void;
  (event: "touch-end", payload: TouchEvent): void;
}>();
</script>

<template>
  <div
    class="relative w-full drag-handle"
    :class="isWechatLayout ? 'mb-0' : 'mb-2'"
  >
    <div
      class="absolute inset-0 overflow-hidden z-0 pointer-events-none"
      :class="isWechatLayout ? 'rounded-none' : 'rounded-xl'"
    >
      <div
        class="absolute inset-0 bg-black/10 dark:bg-white/10 flex items-center justify-start"
      >
        <div
          class="w-[80px] h-full flex items-center justify-center transition-colors cursor-pointer active:bg-black/5 dark:active:bg-white/5 pointer-events-auto"
          :class="
            ownerType === 'agent'
              ? 'text-blue-600/70 dark:text-blue-400/70 hover:text-blue-600 dark:hover:text-blue-400'
              : 'text-purple-600/70 dark:text-purple-400/70 hover:text-purple-600 dark:hover:text-purple-400'
          "
          @click.stop="emit('settings')"
          @touchstart.stop
        >
          <svg
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <circle cx="12" cy="12" r="3"></circle>
            <path
              d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"
            ></path>
          </svg>
        </div>
      </div>
    </div>

    <div
      class="relative flex items-center gap-3 cursor-pointer z-10 w-full origin-center transition-all duration-300"
      :class="[
        isWechatLayout
          ? [
              'px-3 py-2.5 border-b border-[var(--border-color)]',
              selected
                ? 'bg-[var(--accent-bg)]'
                : 'active:bg-black/5 dark:active:bg-white/5',
            ]
          : [
              'p-3 glass-panel rounded-xl border shadow-sm active:opacity-75',
              selected
                ? 'glass-panel-active'
                : 'border-transparent hover:bg-black/5 dark:hover:bg-white/5',
            ],
        swipeActive && isDragging ? 'transition-none' : '',
      ]"
      :style="{ transform: `translateX(${swipeActive ? swipeX : 0}px)` }"
      @click="emit('select')"
      @touchstart="emit('touch-start', $event)"
      @touchmove="emit('touch-move', $event)"
      @touchend="emit('touch-end', $event)"
    >
      <div
        v-if="showUnread"
        class="absolute rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm shrink-0"
        :class="
          isWechatLayout
            ? '-top-0.5 -right-0.5 w-2.5 h-2.5'
            : '-top-1 -right-1 w-3 h-3'
        "
        :style="{ background: isWechatLayout ? '#FA5151' : '#ff6b6b' }"
      ></div>

      <VcpAvatar
        :owner-type="ownerType"
        :owner-id="ownerId"
        :fallback-name="name"
        size="w-10 h-10"
        rounded="rounded-full"
        :class="ownerType === 'agent' ? 'pointer-events-none' : ''"
        dominant-color="var(--highlight-text)"
      />
      <div class="flex flex-col overflow-hidden flex-1 pointer-events-none">
        <span class="font-bold text-sm truncate text-primary-text">{{
          name
        }}</span>
        <!-- 微信布局不暴露模型名 / 群组运行模式这类技术信息 -->
        <span
          v-if="!isWechatLayout"
          class="text-secondary-text opacity-80 truncate"
          :class="
            ownerType === 'group'
              ? 'text-[9px] uppercase tracking-tighter'
              : 'text-[10px]'
          "
          >{{ subtitle }}</span
        >
      </div>
    </div>
  </div>
</template>
