import { useAssistantStore } from "./assistant";
import { useSettingsStore } from "./settings";
import { useThemeStore } from "./theme";
import { useNotificationStore } from "./notification";
import { useChatSessionStore } from "./chatSessionStore";
import { useTopicStore } from "./topicListManager";
import { useAvatarStore } from "./avatar";

export interface AppLifecycleStores {
  assistantStore: ReturnType<typeof useAssistantStore>;
  settingsStore: ReturnType<typeof useSettingsStore>;
  themeStore: ReturnType<typeof useThemeStore>;
  notificationStore: ReturnType<typeof useNotificationStore>;
  avatarStore: ReturnType<typeof useAvatarStore>;
  sessionStore: ReturnType<typeof useChatSessionStore>;
  topicStore: ReturnType<typeof useTopicStore>;
}

export function createAppLifecycleStores(): AppLifecycleStores {
  return {
    assistantStore: useAssistantStore(),
    settingsStore: useSettingsStore(),
    themeStore: useThemeStore(),
    notificationStore: useNotificationStore(),
    avatarStore: useAvatarStore(),
    sessionStore: useChatSessionStore(),
    topicStore: useTopicStore(),
  };
}
