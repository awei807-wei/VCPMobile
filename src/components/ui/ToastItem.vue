<script setup lang="ts">
import { computed, ref } from "vue";
import { useSwipe } from "@vueuse/core";
import {
  Info,
  CheckCircle,
  AlertTriangle,
  X,
  Cpu,
  User,
} from "lucide-vue-next";
import {
  useNotificationStore,
  type VcpNotification,
} from "../../core/stores/notification";

const props = defineProps<{
  toast: VcpNotification;
}>();

const store = useNotificationStore();

const getIcon = (type: string) => {
  switch (type) {
    case "success":
      return CheckCircle;
    case "warning":
      return AlertTriangle;
    case "error":
      return X;
    case "tool":
      return Cpu;
    case "agent":
      return User;
    default:
      return Info;
  }
};

const getIconColor = (type: string) => {
  switch (type) {
    case "success":
      return "text-[var(--success-color)]";
    case "warning":
      return "text-[var(--highlight-text)]";
    case "error":
      return "text-[var(--danger-color)]";
    case "tool":
      return "text-[var(--notification-border)]";
    case "agent":
      return "text-[var(--highlight-text)]";
    default:
      return "text-[var(--highlight-text)]";
  }
};

const dismissToast = (id: string) => {
  store.activeToasts = store.activeToasts.filter(
    (toast: VcpNotification) => toast.id !== id,
  );
};

const el = ref<HTMLElement | null>(null);
const { isSwiping, lengthX } = useSwipe(el, {
  onSwipeEnd(_, direction) {
    if (
      (direction === "left" || direction === "right") &&
      Math.abs(lengthX.value) > 60
    ) {
      dismissToast(props.toast.id);
    }
  },
});

const swipeStyle = computed(() => {
  if (isSwiping.value) {
    const opacity = Math.max(0, 1 - Math.abs(lengthX.value) / 200);
    return {
      transform: `translateX(${-lengthX.value}px)`,
      opacity,
      transition: "none",
    };
  }
  return {
    transform: "translateX(0px)",
    opacity: 1,
    transition:
      "transform 0.25s cubic-bezier(0.16, 1, 0.3, 1), opacity 0.25s ease-out",
  };
});

const handleClick = () => {
  dismissToast(props.toast.id);
};
</script>

<template>
  <div
    ref="el"
    class="vcp-toast-item pointer-events-auto flex items-center justify-between gap-3 px-3.5 py-2.5 rounded-xl backdrop-blur-md w-full max-w-[calc(100vw-32px)] sm:w-[320px] overflow-hidden transition-all active:scale-[0.98] cursor-pointer touch-none select-none no-swipe"
    :style="swipeStyle"
    @click="handleClick"
  >
    <div class="flex items-start gap-3 min-w-0 flex-1">
      <component
        :is="getIcon(toast.type)"
        :size="14"
        :class="getIconColor(toast.type)"
        class="mt-0.5 shrink-0 opacity-80"
      />
      <div class="flex flex-col min-w-0 flex-1">
        <span
          class="text-[11px] font-bold text-primary-text leading-tight tracking-wide truncate"
          >{{ toast.title }}</span
        >

        <p
          v-if="toast.message"
          :class="[
            toast.isPreformatted
              ? 'font-mono text-[9px] opacity-60 leading-normal'
              : 'text-[9.5px] opacity-50 leading-snug',
            'vcp-toast-message text-primary-text break-words mt-0.5 select-text',
          ]"
        >
          {{ toast.message }}
        </p>
      </div>
    </div>

    <button
      @click.stop="dismissToast(toast.id)"
      class="p-1 opacity-20 hover:opacity-100 text-primary-text transition-opacity shrink-0 ml-1 self-start"
      :aria-label="`关闭通知：${toast.title}`"
    >
      <X :size="12" />
    </button>
  </div>
</template>

<style scoped>
.vcp-toast-item {
  color: var(--primary-text);
  background-color: color-mix(
    in srgb,
    var(--notification-bg, var(--secondary-bg)) 94%,
    transparent
  );
  border: 1px solid
    color-mix(
      in srgb,
      var(--notification-border, var(--border-color)) 58%,
      transparent
    );
  box-shadow:
    0 8px 30px rgb(0 0 0 / 14%),
    inset 0 1px 0 color-mix(in srgb, white 8%, transparent);
}

@media (prefers-reduced-motion: reduce) {
  .vcp-toast-item {
    transition: none !important;
  }
}
</style>
