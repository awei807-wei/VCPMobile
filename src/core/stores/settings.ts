import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useNotificationStore } from "./notification";

export const CONNECTION_PROFILE_IDS = ["lan", "wan"] as const;
export type ConnectionProfileId = (typeof CONNECTION_PROFILE_IDS)[number];

export interface ConnectionProfile {
  id: ConnectionProfileId;
  name: string;
  vcpServerUrl: string;
  vcpApiKey: string;
  vcpLogUrl: string;
  vcpLogKey: string;
  syncServerUrl: string;
  syncHttpUrl: string;
  syncToken: string;
  distributedWsUrl: string;
  distributedVcpKey: string;
}

export interface AppSettings {
  userName: string;
  vcpServerUrl: string;
  vcpApiKey: string;
  vcpLogUrl: string;
  vcpLogKey: string;
  syncServerUrl: string;
  syncHttpUrl: string;
  syncToken: string;
  adminUsername?: string;
  adminPassword?: string;
  fileKey?: string;
  topicSummaryModel: string;
  syncLogLevel: string;
  agentOrder: string[];
  groupOrder: string[];
  currentThemeMode?: string;
  syncPrerenderEnabled?: boolean;
  enableAssistant?: boolean;
  assistantAgentId?: string;
  distributedEnabled?: boolean;
  distributedWsUrl?: string;
  distributedVcpKey?: string;
  distributedDeviceName?: string;
  connectionProfiles?: ConnectionProfile[];
  activeConnectionProfileId?: ConnectionProfileId;
  [key: string]: any;
}

export const getDefaultConnectionProfileName = (id: ConnectionProfileId) =>
  id === "lan" ? "内网" : "外网";

export const normalizeConnectionProfileId = (
  id: string | null | undefined,
): ConnectionProfileId => (id === "wan" ? "wan" : "lan");

export const createEmptyConnectionProfile = (
  id: ConnectionProfileId,
): ConnectionProfile => ({
  id,
  name: getDefaultConnectionProfileName(id),
  vcpServerUrl: "",
  vcpApiKey: "",
  vcpLogUrl: "",
  vcpLogKey: "",
  syncServerUrl: "",
  syncHttpUrl: "",
  syncToken: "",
  distributedWsUrl: "",
  distributedVcpKey: "",
});

const CONNECTION_PROFILE_RUNTIME_FIELDS = [
  "vcpServerUrl",
  "vcpApiKey",
  "vcpLogUrl",
  "vcpLogKey",
  "syncServerUrl",
  "syncHttpUrl",
  "syncToken",
  "distributedWsUrl",
  "distributedVcpKey",
] as const satisfies readonly (keyof ConnectionProfile & keyof AppSettings)[];

export type ConnectionProfileRuntimeField =
  (typeof CONNECTION_PROFILE_RUNTIME_FIELDS)[number];

const createConnectionProfileFromSettings = (
  settings: Partial<AppSettings> | null | undefined,
  id: ConnectionProfileId,
): ConnectionProfile => ({
  ...createEmptyConnectionProfile(id),
  vcpServerUrl: settings?.vcpServerUrl || "",
  vcpApiKey: settings?.vcpApiKey || "",
  vcpLogUrl: settings?.vcpLogUrl || "",
  vcpLogKey: settings?.vcpLogKey || "",
  syncServerUrl: settings?.syncServerUrl || "",
  syncHttpUrl: settings?.syncHttpUrl || "",
  syncToken: settings?.syncToken || "",
  distributedWsUrl: settings?.distributedWsUrl || settings?.vcpLogUrl || "",
  distributedVcpKey: settings?.distributedVcpKey || settings?.vcpLogKey || "",
});

export const normalizeConnectionProfiles = (
  settings: Partial<AppSettings> | null | undefined,
): ConnectionProfile[] => {
  const existing = Array.isArray(settings?.connectionProfiles)
    ? settings?.connectionProfiles
    : [];

  return CONNECTION_PROFILE_IDS.map((id) => {
    const profile = existing?.find((item) => item?.id === id);
    if (profile) {
      return {
        ...createEmptyConnectionProfile(id),
        ...profile,
        id,
        name: profile.name || getDefaultConnectionProfileName(id),
      };
    }

    return id === "lan"
      ? createConnectionProfileFromSettings(settings, id)
      : createEmptyConnectionProfile(id);
  });
};

export const ensureConnectionProfiles = (settings: AppSettings) => {
  settings.connectionProfiles = normalizeConnectionProfiles(settings);
  settings.activeConnectionProfileId = normalizeConnectionProfileId(
    settings.activeConnectionProfileId,
  );
  return settings.connectionProfiles;
};

export const copySettingsToConnectionProfile = (
  settings: AppSettings,
  profile: ConnectionProfile,
) => {
  CONNECTION_PROFILE_RUNTIME_FIELDS.forEach((field) => {
    profile[field] = (settings[field] as string | undefined) || "";
  });
};

export const copyConnectionProfileToSettings = (
  settings: AppSettings,
  profile: ConnectionProfile,
) => {
  CONNECTION_PROFILE_RUNTIME_FIELDS.forEach((field) => {
    settings[field] = profile[field] || "";
  });
};

export const syncActiveConnectionProfileFromSettings = (
  settings: AppSettings,
) => {
  const profiles = ensureConnectionProfiles(settings);
  const activeProfileId = normalizeConnectionProfileId(
    settings.activeConnectionProfileId,
  );
  const activeProfile = profiles.find(
    (profile) => profile.id === activeProfileId,
  );

  if (activeProfile) {
    copySettingsToConnectionProfile(settings, activeProfile);
  }

  return activeProfile;
};

export const useSettingsStore = defineStore("settings", () => {
  const settings = ref<AppSettings | null>(null);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const notificationStore = useNotificationStore();

  const fetchSettings = async () => {
    loading.value = true;
    error.value = null;
    try {
      const fetchedSettings = await invoke<AppSettings>("read_settings");
      ensureConnectionProfiles(fetchedSettings);
      settings.value = fetchedSettings;
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to fetch settings:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const saveSettings = async (newSettings: AppSettings) => {
    loading.value = true;
    error.value = null;
    try {
      syncActiveConnectionProfileFromSettings(newSettings);
      await invoke("write_settings", { settings: newSettings });
      settings.value = newSettings;

      notificationStore.addNotification({
        type: "success",
        title: "设置更新成功",
        message: "全局配置已持久化",
        toastOnly: true,
      });
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to save settings:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  const updateSettings = async (updates: Record<string, any>) => {
    loading.value = true;
    error.value = null;
    try {
      const preparedUpdates = { ...updates };
      if (settings.value) {
        const mergedSettings = JSON.parse(JSON.stringify({
          ...settings.value,
          ...updates,
        })) as AppSettings;
        syncActiveConnectionProfileFromSettings(mergedSettings);
        preparedUpdates.connectionProfiles = mergedSettings.connectionProfiles;
        preparedUpdates.activeConnectionProfileId =
          mergedSettings.activeConnectionProfileId;
      }

      const updated = await invoke<AppSettings>("update_settings", {
        updates: preparedUpdates,
      });
      ensureConnectionProfiles(updated);
      settings.value = updated;

      notificationStore.addNotification({
        type: "success",
        title: "配置同步成功",
        message: "变更已生效",
        toastOnly: true,
      });
      return updated;
    } catch (e: any) {
      error.value = e.toString();
      console.error("[SettingsStore] Failed to update settings:", e);
      throw e;
    } finally {
      loading.value = false;
    }
  };

  return {
    settings,
    loading,
    error,
    fetchSettings,
    saveSettings,
    updateSettings,
  };
});
