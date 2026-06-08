import { computed } from "vue";
import { defineStore } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "./settings";
import { useChatStreamStore } from "./chatStreamStore";
import { useSyncSessionStore } from "./syncSession";
import { useNotificationStore } from "./notification";
import { useModelStore } from "./modelStore";
import { useFloatingAssistantActivityStore } from "./floatingAssistantActivity";
import { useConnectionSwitchGuardStore } from "./connectionSwitchGuard";
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

  const hasAnyVcpLog =
    hasValue(profile.vcpLogUrl) || hasValue(profile.vcpLogKey);
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
    (!hasValue(profile.distributedWsUrl) ||
      !hasValue(profile.distributedVcpKey))
  ) {
    return `${getProfileName(profile)}的分布式配置不完整`;
  }

  return null;
};

export const useConnectionProfilesStore = defineStore(
  "connectionProfiles",
  () => {
    const settingsStore = useSettingsStore();
    const chatStreamStore = useChatStreamStore();
    const syncSessionStore = useSyncSessionStore();
    const notificationStore = useNotificationStore();
    const modelStore = useModelStore();
    const floatingAssistantActivityStore = useFloatingAssistantActivityStore();
    const switchGuardStore = useConnectionSwitchGuardStore();
    floatingAssistantActivityStore.ensureListening();

    let activeSwitchPromise: Promise<void> | null = null;

    const profiles = computed(() =>
      normalizeConnectionProfiles(settingsStore.settings),
    );

    const activeProfileId = computed(() =>
      normalizeConnectionProfileId(
        settingsStore.settings?.activeConnectionProfileId,
      ),
    );

    const activeProfile = computed(
      () =>
        profiles.value.find(
          (profile) => profile.id === activeProfileId.value,
        ) || profiles.value[0],
    );

    const targetProfileId = computed<ConnectionProfileId>(() =>
      activeProfileId.value === "lan" ? "wan" : "lan",
    );

    const targetProfile = computed(
      () =>
        profiles.value.find(
          (profile) => profile.id === targetProfileId.value,
        ) || profiles.value[1],
    );

    const activeProfileName = computed(() =>
      getProfileName(activeProfile.value),
    );
    const targetProfileName = computed(() =>
      getProfileName(targetProfile.value),
    );
    const switching = computed(() => switchGuardStore.switching);
    const hasActiveSyncSession = computed(() => syncSessionStore.isActive);
    const hasModelRefreshInFlight = computed(() => modelStore.isLoading);
    const hasFloatingAssistantGenerating = computed(
      () => floatingAssistantActivityStore.isGenerating,
    );
    const canSwitch = computed(
      () =>
        !switching.value &&
        !chatStreamStore.hasActiveStreams &&
        !hasFloatingAssistantGenerating.value &&
        !modelStore.isLoading &&
        !hasActiveSyncSession.value,
    );

    const notifyBlocked = (message: string) => {
      notificationStore.addNotification({
        type: "warning",
        title: "线路切换已阻止",
        message,
        toastOnly: true,
      });
    };

    const formatErrorMessage = (error: unknown, fallback: string) => {
      if (typeof error === "string" && error.trim()) return error;
      if (
        error &&
        typeof error === "object" &&
        "message" in error &&
        typeof error.message === "string" &&
        error.message.trim()
      ) {
        return error.message;
      }
      const text = String(error ?? "").trim();
      return text && text !== "[object Object]" ? text : fallback;
    };

    const notifySwitchFailed = (error: unknown) => {
      notificationStore.addNotification({
        type: "error",
        title: "线路切换失败",
        message: formatErrorMessage(error, "当前线路状态保持不变"),
        toastOnly: true,
      });
    };

    const hasBackendActiveSyncSession = async () => {
      try {
        return await invoke<boolean>("is_sync_active");
      } catch (error) {
        console.error(
          "[ConnectionProfiles] Failed to read sync status:",
          error,
        );
        throw new Error(
          `无法确认数据同步状态：${formatErrorMessage(error, "请稍后重试")}`,
        );
      }
    };

    const hasBackendActiveAssistantGeneration = async () => {
      try {
        return await invoke<boolean>("is_assistant_chat_active");
      } catch (error) {
        console.error(
          "[ConnectionProfiles] Failed to read assistant status:",
          error,
        );
        throw new Error(
          `无法确认划词助手状态：${formatErrorMessage(error, "请稍后重试")}`,
        );
      }
    };

    const readBlocker = async (): Promise<string | null> => {
      if (chatStreamStore.hasActiveStreams) {
        return "输出中不可切换";
      }

      if (hasFloatingAssistantGenerating.value) {
        return "划词助手输出中不可切换";
      }

      if (await hasBackendActiveAssistantGeneration()) {
        return "划词助手输出中不可切换";
      }

      if (modelStore.isLoading) {
        return "模型列表刷新中不可切换，请等待刷新结束后再试";
      }

      if (hasActiveSyncSession.value) {
        return "数据同步中不可切换，请等待同步结束后再试";
      }

      if (await hasBackendActiveSyncSession()) {
        return "数据同步中不可切换，请等待同步结束后再试";
      }

      return null;
    };

    const blockIfBusy = async () => {
      const blocker = await readBlocker();
      if (blocker) {
        notifyBlocked(blocker);
        return true;
      }
      return false;
    };

    const switchTo = async (profileId: ConnectionProfileId) => {
      if (switchGuardStore.switching && activeSwitchPromise) {
        return activeSwitchPromise;
      }

      const run = async () => {
        await switchGuardStore.beginSwitch();
        try {
          if (!settingsStore.settings) {
            await settingsStore.fetchSettings();
          }

          const settings = settingsStore.settings;
          if (!settings) {
            notifyBlocked("设置尚未加载完成，请稍后重试");
            return;
          }

          if (await blockIfBusy()) {
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
          const target = nextProfiles.find(
            (profile) => profile.id === profileId,
          );
          if (!target) {
            notifyBlocked("目标线路不存在，请进入设置页补全");
            return;
          }

          const validationError = validateProfile(target, settings);
          if (validationError) {
            notifyBlocked(`${validationError}，请进入设置页补全`);
            return;
          }

          if (await blockIfBusy()) {
            return;
          }

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

          try {
            await modelStore.invalidatePersistedCache();
          } catch (error) {
            console.error(
              "[ConnectionProfiles] Failed to invalidate model cache after committed switch:",
              error,
            );
            notificationStore.addNotification({
              type: "warning",
              title: "模型缓存清理失败",
              message: `线路已切换到${getProfileName(target)}，但旧模型缓存清理失败；下次刷新会重新拉取`,
              toastOnly: true,
            });
          }

          notificationStore.addNotification({
            type: "success",
            title: "线路已切换",
            message: `当前线路：${getProfileName(target)}，模型列表将在下次刷新时更新`,
            toastOnly: true,
          });
        } catch (error: any) {
          notifySwitchFailed(error);
          throw error;
        } finally {
          await switchGuardStore.endSwitch();
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
      hasActiveSyncSession,
      hasModelRefreshInFlight,
      hasFloatingAssistantGenerating,
      canSwitch,
      switchTo,
      switchToTarget,
    };
  },
);
