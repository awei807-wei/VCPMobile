<script setup lang="ts">
import { computed } from "vue";
import { useNotificationStore } from "../../core/stores/notification";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useThemeStore } from "../../core/stores/theme";

const notificationStore = useNotificationStore();
const streamStore = useChatStreamStore();
const themeStore = useThemeStore();

const isWechat = computed(() => themeStore.isWechatLayout);

/**
 * 微信伪装布局的副标题：只保留「对方正在输入...」与「未连接」两种人话，
 * 空闲时返回空串，让顶栏像原生微信单聊一样只显示联系人名字。
 */
const wechatText = computed(() => {
  if (streamStore.activeStreamingIds.size > 0) return "对方正在输入...";
  if (notificationStore.vcpStatus.status !== "connected") return "未连接";
  if (notificationStore.vcpCoreStatus.status === "error") return "未连接";
  return "";
});

const statusConfig = computed(() => {
  // P0 优先级：VCPLog 未连接时，覆盖 Core 状态显示红灯
  const logStatus = notificationStore.vcpStatus.status;
  if (logStatus !== "connected") {
    return {
      color: "bg-red-500",
      shadow: "shadow-red-500/50",
      animate: "animate-pulse",
      text: "VCPLog 未连接",
    };
  }

  // 正常模式：显示 Core 引擎状态
  const s = notificationStore.vcpCoreStatus.status;
  switch (s) {
    case "ready":
      return {
        color: "bg-green-500",
        shadow: "shadow-green-500/50",
        animate: "vcp-core-pulse",
        text: "Core Active",
      };
    case "initializing":
    case "connecting":
      return {
        color: "bg-yellow-500",
        shadow: "shadow-yellow-500/50",
        animate: "animate-pulse",
        text: "Booting...",
      };
    case "error":
      return {
        color: "bg-red-500",
        shadow: "shadow-red-500/50",
        animate: "animate-bounce",
        text: "Core Error",
      };
    default:
      return {
        color: "bg-gray-400",
        shadow: "shadow-gray-400/20",
        animate: "",
        text: "Unknown",
      };
  }
});
</script>

<template>
  <div
    v-if="!isWechat || wechatText"
    class="flex items-center gap-1.5 transition-all duration-300 select-none"
    :title="notificationStore.vcpCoreStatus.message"
  >
    <template v-if="isWechat">
      <span class="text-[11px] text-[var(--secondary-text)]">{{
        wechatText
      }}</span>
    </template>
    <template v-else>
      <!-- 呼吸灯 -->
      <div
        class="w-1.5 h-1.5 rounded-full transition-colors duration-500"
        :class="[statusConfig.color, statusConfig.shadow, statusConfig.animate]"
      ></div>

      <!-- 状态文字 -->
      <span class="text-[9px] opacity-40 uppercase font-mono tracking-tighter">
        {{ statusConfig.text }}
      </span>
    </template>
  </div>
</template>

<style scoped>
.vcp-core-pulse {
  animation: vcpCorePulse 2s cubic-bezier(0.4, 0, 0.2, 1) infinite;
  /* 提升至 GPU 合成层 */
  will-change: transform, opacity;
  transform: translate3d(0, 0, 0);
}

@keyframes vcpCorePulse {
  0%,
  100% {
    opacity: 1;
    transform: scale(1.1) translate3d(0, 0, 0);
  }
  50% {
    opacity: 0.6;
    transform: scale(0.9) translate3d(0, 0, 0);
  }
}
</style>
