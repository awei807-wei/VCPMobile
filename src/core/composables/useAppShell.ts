import { onMounted, onUnmounted, ref, type Ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useRouter } from "vue-router";
import { useSidebarSwipe } from "./useSidebarSwipe";
import { useWindowInsets } from "./useWindowInsets";
import { useAppLifecycle } from "./useAppLifecycle";
import { useAppLifecycleStore } from "../stores/appLifecycle";
import { useAssistantStore } from "../stores/assistant";
import { useChatSessionStore } from "../stores/chatSessionStore";
import { useLayoutStore } from "../stores/layout";
import {
  useNotificationStore,
  type VcpNotification,
} from "../stores/notification";
import { useSettingsStore } from "../stores/settings";
import { useThemeStore } from "../stores/theme";
import { useAutoUpdate } from "./useAutoUpdate";
import { useEmoticonFixer } from "./useEmoticonFixer";
import { useModalHistory } from "./useModalHistory";
import { useNotificationProcessor } from "./useNotificationProcessor";
import { findAgentMessagePayload } from "../utils/agentMessagePayload";
import { useAppBackgroundStyle } from "./useAppBackgroundStyle";
import { useAppNotificationRouting } from "./useAppNotificationRouting";
import { useAppShareIntent } from "./useAppShareIntent";
import { useAppWindowLifecycle } from "./useAppWindowLifecycle";

const createAppShellContext = () => {
  useAppLifecycle();
  const themeStore = useThemeStore();
  const lifecycleStore = useAppLifecycleStore();
  const notificationStore = useNotificationStore();
  const layoutStore = useLayoutStore();
  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const settingsStore = useSettingsStore();
  const router = useRouter();
  const modalHistory = useModalHistory();
  const { processPayload } = useNotificationProcessor();
  const { initGlobalFixer } = useEmoticonFixer();
  const update = useAutoUpdate();
  const appRootRef = ref<HTMLElement | null>(null);
  useSidebarSwipe(appRootRef, { type: "global" });
  useWindowInsets();
  return {
    themeStore,
    lifecycleStore,
    notificationStore,
    layoutStore,
    sessionStore,
    assistantStore,
    settingsStore,
    router,
    modalHistory,
    processPayload,
    initGlobalFixer,
    appRootRef,
    update,
  };
};

type AppShellContext = ReturnType<typeof createAppShellContext>;

const createAppShellIntegrations = (context: AppShellContext) => ({
  windowLifecycle: useAppWindowLifecycle({
    lifecycleStore: context.lifecycleStore,
    sessionStore: context.sessionStore,
    notificationStore: context.notificationStore,
    modalHistory: context.modalHistory,
  }),
  shareIntent: useAppShareIntent({
    lifecycleStore: context.lifecycleStore,
    assistantStore: context.assistantStore,
    sessionStore: context.sessionStore,
  }),
  notificationRouting: useAppNotificationRouting({
    lifecycleStore: context.lifecycleStore,
    layoutStore: context.layoutStore,
    sessionStore: context.sessionStore,
    modalHistory: context.modalHistory,
  }),
  backgroundStyle: useAppBackgroundStyle(context.themeStore),
});

type AppShellIntegrations = ReturnType<typeof createAppShellIntegrations>;

const pushAgentSystemNotification = async (
  notification: Partial<VcpNotification>,
  rawPayload?: any,
) => {
  const agentPayload = findAgentMessagePayload(
    rawPayload ?? notification.rawPayload,
  );
  if (!agentPayload && notification.type !== "agent") return;
  if (agentPayload?.androidNotification?.delivered === true) return;
  const title = String(
    agentPayload?.title ||
      (agentPayload?.recipient
        ? `${agentPayload.recipient} 的消息`
        : notification.title || "Agent 消息"),
  );
  const body = String(
    agentPayload?.originalContent ||
      agentPayload?.message ||
      notification.message ||
      "",
  );
  if (!body.trim()) return;
  try {
    await invoke("plugin:vcp-mobile|show_system_notification", { title, body });
    console.info("[App] Agent system notification submitted:", {
      title,
      bodyLength: body.length,
    });
  } catch (error) {
    console.warn("[App] Agent system notification push failed:", error);
  }
};

const createSystemEventHandler = (
  context: AppShellContext,
): ((event: any) => void) => {
  return (event: any) => {
    const processed = context.processPayload(event.payload);
    if (!processed || processed.silent) return;
    void pushAgentSystemNotification(processed, event.payload);
    context.notificationStore.addNotification(processed);
  };
};

const bootstrapApp = async (
  context: AppShellContext,
  isAssistant: Ref<boolean>,
) => {
  try {
    if (isAssistant.value) {
      context.lifecycleStore.state = "READY";
      await context.themeStore.initTheme();
      context.settingsStore.fetchSettings().catch(() => {});
      return;
    }
    await context.lifecycleStore.bootstrap();
  } catch (error) {
    console.error("[App] Bootstrap failed:", error);
  }
};

const registerWindowListeners = (integrations: AppShellIntegrations) => {
  const { windowLifecycle, shareIntent, notificationRouting } = integrations;
  window.addEventListener(
    "vcp-exit-requested",
    windowLifecycle.handleExitRequest,
  );
  window.addEventListener(
    "vcp-hardware-back",
    windowLifecycle.handleExitRequest,
  );
  document.addEventListener(
    "visibilitychange",
    windowLifecycle.handleVisibilityChange,
  );
  window.addEventListener("vcp-lifecycle", windowLifecycle.handleVcpLifecycle);
  window.addEventListener(
    "vcp-floating-ball-click",
    windowLifecycle.handleFloatingBallClick,
  );
  window.addEventListener("vcp-share-intent", shareIntent.handleShareIntent);
  window.addEventListener(
    "vcp-notification-click",
    notificationRouting.handleNotificationClick,
  );
};

const removeWindowListeners = (integrations: AppShellIntegrations) => {
  const { windowLifecycle, shareIntent, notificationRouting } = integrations;
  window.removeEventListener(
    "vcp-exit-requested",
    windowLifecycle.handleExitRequest,
  );
  window.removeEventListener(
    "vcp-hardware-back",
    windowLifecycle.handleExitRequest,
  );
  document.removeEventListener(
    "visibilitychange",
    windowLifecycle.handleVisibilityChange,
  );
  window.removeEventListener(
    "vcp-lifecycle",
    windowLifecycle.handleVcpLifecycle,
  );
  window.removeEventListener(
    "vcp-floating-ball-click",
    windowLifecycle.handleFloatingBallClick,
  );
  window.removeEventListener("vcp-share-intent", shareIntent.handleShareIntent);
  window.removeEventListener(
    "vcp-notification-click",
    notificationRouting.handleNotificationClick,
  );
};

const handlePendingNotification = async (
  integrations: AppShellIntegrations,
) => {
  if (integrations.windowLifecycle.isAssistant.value) return;
  try {
    const pending = await invoke<any>(
      "plugin:vcp-mobile|get_pending_notification",
    );
    if (pending?.topicId) {
      integrations.notificationRouting.processNotificationClick(pending);
    }
  } catch (error) {
    console.warn("[App] Failed to fetch pending notification click:", error);
  }
};

export function useAppShell() {
  const context = createAppShellContext();
  const integrations = createAppShellIntegrations(context);
  let unlistenLog: (() => void) | null = null;
  let disposed = false;
  const systemEventHandler = createSystemEventHandler(context);

  onMounted(async () => {
    integrations.windowLifecycle.detectAssistantWindow();
    registerWindowListeners(integrations);
    context.initGlobalFixer();
    const stopListening = await listen("vcp-system-event", systemEventHandler);
    if (disposed) {
      stopListening();
      return;
    }
    unlistenLog = stopListening;
    await bootstrapApp(context, integrations.windowLifecycle.isAssistant);
    await context.router.isReady();
    context.modalHistory.initRootHistory();
    context.router.afterEach(() => context.modalHistory.initRootHistory());
    await handlePendingNotification(integrations);
  });

  onUnmounted(() => {
    disposed = true;
    unlistenLog?.();
    unlistenLog = null;
    removeWindowListeners(integrations);
  });

  return {
    appRootRef: context.appRootRef,
    lifecycleStore: context.lifecycleStore,
    themeStore: context.themeStore,
    layoutStore: context.layoutStore,
    backgroundStyle: integrations.backgroundStyle,
    ...integrations.windowLifecycle,
    ...integrations.shareIntent,
    isPromptOpen: context.update.isPromptOpen,
    updateInfo: context.update.updateInfo,
    handleConfirm: context.update.handleConfirm,
    handleDismiss: context.update.handleDismiss,
  };
}
