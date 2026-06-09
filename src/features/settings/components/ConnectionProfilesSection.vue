<script setup lang="ts">
import { computed } from "vue";
import {
  copyConnectionProfileToSettings,
  copySettingsToConnectionProfile,
  ensureConnectionProfiles,
  getDefaultConnectionProfileName,
  type AppSettings,
  type ConnectionProfile,
  type ConnectionProfileRuntimeField,
} from "../../../core/stores/settings";
import SettingsTextField from "../../../components/settings/SettingsTextField.vue";
import SettingsActionButton from "../../../components/settings/SettingsActionButton.vue";

const props = defineProps<{
  settings: AppSettings;
}>();

const emit = defineEmits<{
  (e: "save-request"): void;
}>();

const profiles = computed(() => ensureConnectionProfiles(props.settings));

const activeProfileId = computed(() => props.settings.activeConnectionProfileId ?? "lan");

const profileTitle = (profile: ConnectionProfile) =>
  profile.name || getDefaultConnectionProfileName(profile.id);

const copyCurrentSettingsToProfile = (profile: ConnectionProfile) => {
  copySettingsToConnectionProfile(props.settings, profile);
  emit("save-request");
};

const updateProfileField = (
  profile: ConnectionProfile,
  field: ConnectionProfileRuntimeField,
  value: string,
) => {
  profile[field] = value || "";
  if (profile.id === activeProfileId.value) {
    copyConnectionProfileToSettings(props.settings, profile);
  }
};
</script>

<template>
  <div class="space-y-5 px-1">
    <div class="rounded-2xl bg-black/5 dark:bg-white/5 px-3 py-2 text-[11px] font-bold text-primary-text">
      <span class="opacity-45">当前线路</span>
      <span class="ml-2">
        {{ getDefaultConnectionProfileName(activeProfileId) }}
      </span>
    </div>

    <section
      v-for="profile in profiles"
      :key="profile.id"
      class="space-y-4 border-t border-black/5 dark:border-white/5 pt-5 first:border-t-0 first:pt-0"
    >
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <div class="text-[13px] font-black text-primary-text">
            {{ profileTitle(profile) }}
          </div>
          <div class="text-[10px] opacity-40 mt-0.5">
            {{ profile.id === activeProfileId ? "当前线路" : "备用线路" }}
          </div>
        </div>
        <SettingsActionButton
          variant="secondary"
          size="sm"
          @click="copyCurrentSettingsToProfile(profile)"
        >
          复制当前
        </SettingsActionButton>
      </div>

      <SettingsTextField
        v-model="profile.name"
        label="线路名称"
        :placeholder="getDefaultConnectionProfileName(profile.id)"
      />

      <div class="space-y-3">
        <div class="text-[10px] uppercase font-black opacity-35 px-1">
          核心 HTTP
        </div>
        <SettingsTextField
          :model-value="profile.vcpServerUrl"
          label="VCP 服务器 URL"
          placeholder="https://vcp-endpoint.com"
          mono
          @update:model-value="updateProfileField(profile, 'vcpServerUrl', $event)"
        />
        <SettingsTextField
          :model-value="profile.vcpApiKey"
          is-secure
          label="VCP API Key"
          placeholder="输入 API Key"
          @update:model-value="updateProfileField(profile, 'vcpApiKey', $event)"
        />
      </div>

      <div class="space-y-3">
        <div class="text-[10px] uppercase font-black opacity-35 px-1">
          VCPLog
        </div>
        <SettingsTextField
          :model-value="profile.vcpLogUrl"
          label="WebSocket URL"
          placeholder="ws://localhost:6005"
          mono
          @update:model-value="updateProfileField(profile, 'vcpLogUrl', $event)"
        />
        <SettingsTextField
          :model-value="profile.vcpLogKey"
          is-secure
          label="WebSocket Key"
          placeholder="输入 WebSocket Key"
          mono
          @update:model-value="updateProfileField(profile, 'vcpLogKey', $event)"
        />
      </div>

      <div class="space-y-3">
        <div class="text-[10px] uppercase font-black opacity-35 px-1">
          数据同步
        </div>
        <SettingsTextField
          :model-value="profile.syncHttpUrl"
          label="HTTP 服务 URL"
          placeholder="http://192.168.x.x:5974"
          mono
          @update:model-value="updateProfileField(profile, 'syncHttpUrl', $event)"
        />
        <SettingsTextField
          :model-value="profile.syncServerUrl"
          label="WebSocket 服务 URL"
          placeholder="ws://192.168.x.x:5975"
          mono
          @update:model-value="updateProfileField(profile, 'syncServerUrl', $event)"
        />
        <SettingsTextField
          :model-value="profile.syncToken"
          is-secure
          label="Mobile Sync Token"
          placeholder="输入同步 Token"
          mono
          @update:model-value="updateProfileField(profile, 'syncToken', $event)"
        />
      </div>

      <div class="space-y-3">
        <div class="text-[10px] uppercase font-black opacity-35 px-1">
          分布式
        </div>
        <div class="rounded-2xl bg-black/5 dark:bg-white/5 px-3 py-2 text-[11px] leading-relaxed text-primary-text">
          <div class="opacity-60">
            分布式节点复用上方 VCPLog 的 WebSocket 基地址和 Key，连接时会自动切换到
            <span class="font-mono">/vcp-distributed-server</span>
            通道。
          </div>
          <div class="mt-1 font-mono opacity-40">
            WS: {{ profile.vcpLogUrl || "未配置" }}
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
