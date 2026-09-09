<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { useSettingsStore } from "../../../core/stores/settings";
import { useNotificationStore } from "../../../core/stores/notification";
import { useDistributed } from "../composables/useDistributed";
import { useDistributedRootAccess } from "../composables/useDistributedRootAccess";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";
import SettingsCard from "../../../components/settings/SettingsCard.vue";

defineProps<{ toolCount: number }>();

const settingsStore = useSettingsStore();
const notificationStore = useNotificationStore();
const { status, activate, deactivate } = useDistributed();
const { rootGranted, check: checkRoot, openManager, dispose: disposeRoot } =
  useDistributedRootAccess();

const deviceName = ref("VCPMobile");
const wsUrl = ref("");
const vcpKey = ref("");

function loadLocalSettings(): void {
  const settings = settingsStore.settings;
  if (!settings) return;
  deviceName.value = settings.distributedDeviceName || "VCPMobile";
  // vcpLogUrl/vcpLogKey 是活动连接配置的 SSOT，distributed 字段只作镜像。
  wsUrl.value = settings.vcpLogUrl || "";
  vcpKey.value = settings.vcpLogKey || "";
}

async function loadSettings(): Promise<void> {
  try {
    await settingsStore.fetchSettings();
    loadLocalSettings();
  } catch (error) {
    console.error("[DistributedConnection] 加载连接配置失败：", error);
    notificationStore.addNotification({
      type: "error",
      title: "加载连接配置失败",
      message: "无法读取当前活动连接配置。",
      toastOnly: true,
    });
  }
}

function notifyPersistFailure(action: string, error: unknown): void {
  console.error(`[DistributedConnection] ${action}持久化失败：`, error);
  notificationStore.addNotification({
    type: "error",
    title: `${action}失败`,
    message: "后端未确认配置变更，当前连接状态保持不变。",
    toastOnly: true,
  });
}

async function handleConnect(): Promise<void> {
  if (!wsUrl.value.trim() || !vcpKey.value.trim() || !settingsStore.settings) {
    notificationStore.addNotification({
      type: "warning",
      title: "连接信息不完整",
      message: "请填写 VCPLog 地址和鉴权 Key。",
      toastOnly: true,
    });
    return;
  }
  try {
    await settingsStore.updateSettings({
      vcpLogUrl: wsUrl.value.trim(),
      vcpLogKey: vcpKey.value,
      distributedWsUrl: wsUrl.value.trim(),
      distributedVcpKey: vcpKey.value,
      distributedDeviceName: deviceName.value.trim() || "VCPMobile",
      distributedEnabled: true,
    });
  } catch (error) {
    notifyPersistFailure("连接配置", error);
  }
}

async function handleDisconnect(): Promise<void> {
  if (!settingsStore.settings) return;
  try {
    await settingsStore.updateSettings({ distributedEnabled: false });
  } catch (error) {
    notifyPersistFailure("断开连接", error);
  }
}

async function copyText(value: string, title: string): Promise<void> {
  if (!value) return;
  try {
    await navigator.clipboard.writeText(value);
    notificationStore.addNotification({
      type: "success",
      title,
      message: value,
      toastOnly: true,
    });
  } catch (error) {
    console.error("[DistributedConnection] 复制失败：", error);
  }
}

onMounted(() => {
  activate();
  void loadSettings();
});

onUnmounted(() => {
  deactivate();
  disposeRoot();
});
</script>

<template>
  <div class="px-4 py-6 space-y-6">
    <div class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/5 p-4 rounded-2xl flex items-center justify-between">
      <div class="flex items-center gap-3 min-w-0">
        <span
          class="h-3 w-3 rounded-full shrink-0"
          :class="status.connected ? 'bg-emerald-500' : status.state === 'connecting' || status.state === 'disconnecting' ? 'bg-amber-500' : 'bg-rose-500'"
        ></span>
        <div class="flex flex-col min-w-0">
          <span class="text-xs font-bold">连接状态</span>
          <span class="text-[9px] opacity-50 font-mono uppercase">{{ status.state }}</span>
        </div>
      </div>
      <button
        v-if="status.connected"
        class="px-4 py-1.5 bg-red-500/20 text-red-500 text-xs font-bold rounded-xl disabled:opacity-50"
        :disabled="status.state === 'disconnecting'"
        @click="handleDisconnect"
      >
        断开连接
      </button>
      <button
        v-else
        class="px-4 py-1.5 text-white text-xs font-bold rounded-xl disabled:opacity-50"
        style="background-color: var(--highlight-text)"
        :disabled="status.state === 'connecting'"
        @click="handleConnect"
      >
        {{ status.state === "connecting" ? "连接中…" : "连接" }}
      </button>
    </div>

    <div class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/5 p-4 rounded-2xl space-y-3">
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <span class="text-xs font-bold block">超级用户 Root 状态</span>
          <span class="text-[8px] opacity-40 font-mono uppercase">
            {{ rootGranted === true ? "已获得 Root 权限" : rootGranted === false ? "未获得 Root 权限" : "尚未检测 Root 状态" }}
          </span>
        </div>
        <div class="flex gap-1.5 shrink-0">
          <button
            class="px-2.5 py-1 text-white text-[9px] font-bold rounded-lg"
            style="background-color: var(--highlight-text)"
            @click="checkRoot"
          >
            {{ rootGranted === null ? "检测 Root" : "重新检测" }}
          </button>
          <button
            v-if="rootGranted === false"
            class="px-2.5 py-1 bg-black/10 dark:bg-white/10 text-[9px] font-bold rounded-lg"
            @click="openManager"
          >
            一键授权
          </button>
        </div>
      </div>
      <p class="text-[10px] opacity-60 leading-relaxed">
        Root 可解锁部分底层物理遥测；未授权时相关工具会按系统 API 降级。
      </p>
    </div>

    <SettingsCard>
      <div class="space-y-4 py-1">
        <SettingsTextField
          v-model="deviceName"
          label="节点名称 / Device Name"
          placeholder="例如 VCPMobile"
          :disabled="status.state !== 'disconnected'"
        />
        <SettingsTextField
          v-model="wsUrl"
          label="VCPLog / 分布式 WS 基地址"
          placeholder="ws://192.168.x.x:port"
          mono
          :disabled="status.state !== 'disconnected'"
        />
        <SettingsTextField
          v-model="vcpKey"
          label="VCPLog / 分布式鉴权 Key"
          placeholder="授权校验密钥"
          is-secure
          :disabled="status.state !== 'disconnected'"
        />
      </div>
    </SettingsCard>

    <div v-if="status.connected" class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/5 p-4 rounded-2xl space-y-3">
      <div class="flex justify-between items-center text-xs border-b border-black/5 dark:border-white/5 pb-2">
        <span class="opacity-50">本地客户端 ID</span>
        <button class="font-mono text-[10px]" @click="copyText(status.client_id || '', '客户端 ID 已复制')">
          {{ status.client_id || "N/A" }}
        </button>
      </div>
      <div class="flex justify-between items-center text-xs border-b border-black/5 dark:border-white/5 pb-2">
        <span class="opacity-50">服务端连接 ID</span>
        <button class="font-mono text-[10px]" @click="copyText(status.server_id || '', '服务端 ID 已复制')">
          {{ status.server_id || "N/A" }}
        </button>
      </div>
      <div class="flex justify-between items-center text-xs">
        <span class="opacity-50">已注册分布式工具</span>
        <span class="font-mono font-bold">{{ status.registered_tools }} / {{ toolCount }}</span>
      </div>
    </div>

    <div v-if="status.last_error" class="bg-red-500/10 border border-red-500/20 p-4 rounded-2xl">
      <div class="text-red-500 text-xs font-bold mb-2">错误日志</div>
      <p class="text-[10px] font-mono text-red-500 opacity-80 whitespace-pre-wrap break-all">{{ status.last_error }}</p>
    </div>
  </div>
</template>
