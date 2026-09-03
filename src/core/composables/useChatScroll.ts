import { watch } from "vue";
import {
  ChatScrollController,
  type UseChatScrollOptions,
} from "./chatScrollController";

/** 为 ChatView 连接滚动状态机，并保留原有公开接口。 */
export function useChatScroll(options: UseChatScrollOptions) {
  const controller = new ChatScrollController(options);
  const stopWatchingList = watch(
    options.messageListRef,
    (element, previous) =>
      controller.attachMessageList(element, previous ?? null),
    { immediate: true },
  );
  const stopWatchingLoading = watch(options.isLoadingHistory, (loading) => {
    void controller.handleLoadingChange(loading);
  });
  const stopWatchingCount = watch(options.messageCount, (messageCount) => {
    controller.handleMessageCountChange(messageCount);
  });

  const dispose = () => {
    stopWatchingList();
    stopWatchingLoading();
    stopWatchingCount();
    controller.dispose();
  };

  return {
    showScrollToBottom: controller.showScrollToBottom,
    scrollToBottom: (smooth = false) => controller.scrollToBottom(smooth),
    startAutoScroll: () => undefined,
    stopAutoScroll: () => undefined,
    checkAndLoadMore: () => controller.checkAndLoadMore(),
    reset: () => controller.reset(),
    dispose,
  };
}
