import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { useSettingsStore } from "./settings";
import { useChatStreamStore } from "./chatStreamStore";
import { useNotificationStore } from "./notification";
import { useModelStore } from "./modelStore";
import {
  normalizeConnectionProfileId,
  normalizeConnectionProfiles,
  syncActiveConnectionProfileFromSettings,
  type AppSettings,
  type ConnectionProfile,
  type ConnectionProfileId,
} from "./settings";

const hasValue = (value: string | null | undefined) => !!value?.trim();

const getProfileName = (profile: ConnectionProfile | null | undefined) =>
  profile?.name?.trim() || (profile?.id === "wan" ? "外网" : "内网");

const validateProfile = (
  profile: ConnectionProfile,
  settings: AppSettings,
): string | null => {
  if (!hasValue(profile.vcpServerUrl) || !hasValue(profile.vcpApiKey)) {
    return `${getProfileName(profile)}缺少 VCP 服务器 URL 或 API Key`;
  }

  const hasAnyVcpLog = hasValue(profile.vcpLogUrl) || hasValue(profile.vcpLogKey);
  if (
    hasAnyVcpLog &&
    (!hasValue(profile.vcpLogUrl) || !hasValue(profile.vcpLogKey))
  ) {
    return `${getProfileName(profile)}的 VCPLog 配置不完整`;
  }

  const syncFields = [
    profile.syncHttpUrl,
    profile.syncServerUrl,
    profile.syncToken,
  ];
  const hasAnySync = syncFields.some(hasValue);
  if (hasAnySync && !syncFields.every(hasValue)) {
    return `${getProfileName(profile)}的数据同步配置不完整`;
  }

  const hasAnyDistributed =
    hasValue(profile.distributedWsUrl) || hasValue(profile.distributedVcpKey);
  if (
    (settings.distributedEnabled || hasAnyDistributed) &&
    (!hasValue(profile.distributedWsUrl) || !hasValue(profile.distributedVcpKey))
  ) {
    return `${getProfileName(profile)}的分布式配置不完整`;
  }

  return null;
};

export const useConnectionProfilesStore = defineStore("connectionProfiles", () => {
  const settingsStore = useSettingsStore();
  const chatStreamStore = useChatStreamStore();
  const notificationStore = useNotificationStore();
  const modelStore = useModelStore();

  const switching = ref(false);
  let activeSwitchPromise: Promise<void> | null = null;

  const profiles = computed(() =>
    normalizeConnectionProfiles(settingsStore.settings),
  );

  const activeProfileId = computed(() =>
    normalizeConnectionProfileId(settingsStore.settings?.activeConnectionProfileId),
  );

  const activeProfile = computed(() =>
    profiles.value.find((profile) => profile.id === activeProfileId.value) ||
    profiles.value[0],
  );

  const targetProfileId = computed<ConnectionProfileId>(() =>
    activeProfileId.value === "lan" ? "wan" : "lan",
  );

  const targetProfile = computed(() =>
    profiles.value.find((profile) => profile.id === targetProfileId.value) ||
    profiles.value[1],
  );

  const activeProfileName = computed(() => getProfileName(activeProfile.value));
  const targetProfileName = computed(() => getProfileName(targetProfile.value));
  const canSwitch = computed(
    () => !switching.value && !chatStreamStore.hasActiveStreams,
  );

  const notifyBlocked = (message: string) => {
    notificationStore.addNotification({
      type: "warning",
      title: "线路切换已阻止",
      message,
      toastOnly: true,
    });
  };

  const switchTo = async (profileId: ConnectionProfileId) => {
    if (switching.value && activeSwitchPromise) {
      return activeSwitchPromise;
    }

    const run = async () => {
      if (!settingsStore.settings) {
        await settingsStore.fetchSettings();
      }

      const settings = settingsStore.settings;
      if (!settings) {
        notifyBlocked("设置尚未加载完成，请稍后重试");
        return;
      }

      if (chatStreamStore.hasActiveStreams) {
        notifyBlocked("输出中不可切换");
        return;
      }

      const currentProfileId = normalizeConnectionProfileId(
        settings.activeConnectionProfileId,
      );
      if (profileId === currentProfileId) {
        return;
      }

      syncActiveConnectionProfileFromSettings(settings);
      const nextProfiles = normalizeConnectionProfiles(settings);
      const target = nextProfiles.find((profile) => profile.id === profileId);
      if (!target) {
        notifyBlocked("目标线路不存在，请进入设置页补全");
        return;
      }

      const validationError = validateProfile(target, settings);
      if (validationError) {
        notifyBlocked(`${validationError}，请进入设置页补全`);
        return;
      }

      switching.value = true;
      try {
        await settingsStore.updateSettings({
          connectionProfiles: nextProfiles,
          activeConnectionProfileId: profileId,
          vcpServerUrl: target.vcpServerUrl,
          vcpApiKey: target.vcpApiKey,
          vcpLogUrl: target.vcpLogUrl,
          vcpLogKey: target.vcpLogKey,
          syncServerUrl: target.syncServerUrl,
          syncHttpUrl: target.syncHttpUrl,
          syncToken: target.syncToken,
          distributedWsUrl: target.distributedWsUrl,
          distributedVcpKey: target.distributedVcpKey,
        });

        modelStore.markModelsStale();
        notificationStore.addNotification({
          type: "success",
          title: "线路已切换",
          message: `当前线路：${getProfileName(target)}，模型列表将在下次刷新时更新`,
          toastOnly: true,
        });
      } catch (error: any) {
        notificationStore.addNotification({
          type: "error",
          title: "线路切换失败",
          message: error?.toString?.() || "当前线路状态保持不变",
          toastOnly: true,
        });
        throw error;
      } finally {
        switching.value = false;
      }
    };

    activeSwitchPromise = run().finally(() => {
      activeSwitchPromise = null;
    });
    return activeSwitchPromise;
  };

  const switchToTarget = () => switchTo(targetProfileId.value);

  return {
    switching,
    profiles,
    activeProfileId,
    activeProfile,
    activeProfileName,
    targetProfileId,
    targetProfile,
    targetProfileName,
    canSwitch,
    switchTo,
    switchToTarget,
  };
});
