import { watch } from "vue";
import type { useAppLifecycleStore } from "../stores/appLifecycle";
import type { useChatSessionStore } from "../stores/chatSessionStore";
import type { useLayoutStore } from "../stores/layout";
import type { useModalHistory } from "./useModalHistory";

type LifecycleStore = ReturnType<typeof useAppLifecycleStore>;
type LayoutStore = ReturnType<typeof useLayoutStore>;
type SessionStore = ReturnType<typeof useChatSessionStore>;
type ModalHistory = ReturnType<typeof useModalHistory>;

interface NotificationRoutingOptions {
  lifecycleStore: LifecycleStore;
  layoutStore: LayoutStore;
  sessionStore: SessionStore;
  modalHistory: ModalHistory;
}

export function useAppNotificationRouting({
  lifecycleStore,
  layoutStore,
  sessionStore,
  modalHistory,
}: NotificationRoutingOptions) {
  const processNotificationClick = (detail: any) => {
    console.log("[App] Notification click received:", detail);
    if (!detail?.ownerId || !detail?.topicId) return;
    if (lifecycleStore.state !== "READY") {
      console.log(
        "[App] Core not ready yet, deferring notification click routing...",
      );
      const unwatch = watch(
        () => lifecycleStore.state,
        (state) => {
          if (state !== "READY") return;
          unwatch();
          processNotificationClick(detail);
        },
      );
      return;
    }

    while (modalHistory.modalStackLength() > 0) {
      modalHistory.closeTopModal();
    }
    layoutStore.setLeftDrawer(false);
    layoutStore.setRightDrawer(false);
    if (detail.ownerType !== "agent" && detail.ownerType !== "group") {
      console.error("Notification topic is missing a valid ownerType");
      return;
    }
    void sessionStore
      .selectTopicById(detail.ownerId, detail.ownerType, detail.topicId)
      .catch((error) => {
        console.error("[App] Failed to route notification topic:", error);
      });
  };

  const handleNotificationClick = (event: Event) => {
    processNotificationClick((event as CustomEvent).detail);
  };

  return { handleNotificationClick, processNotificationClick };
}
