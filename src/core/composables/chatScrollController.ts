import { nextTick, ref, type ComputedRef, type Ref } from "vue";

export interface UseChatScrollOptions {
  messageListRef: Ref<HTMLElement | null>;
  messageCount: ComputedRef<number>;
  hasMoreHistory: Ref<boolean>;
  isLoadingHistory: Ref<boolean>;
  onLoadMore: () => void;
}

type ScrollScene = "initial" | "following" | "free" | "loading-top";

interface LoadAnchor {
  messageId: string;
  offsetFromTop: number;
}

/** 管理聊天滚动状态、历史锚点和布局观察器。 */
export class ChatScrollController {
  readonly showScrollToBottom = ref(false);

  private readonly scrollScene = ref<ScrollScene>("initial");
  private readonly isInitialRendering = ref(true);
  private lastScrollHeight = 0;
  private loadAnchor: LoadAnchor | null = null;
  private scrollThrottleId: number | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private scrollRafId: number | null = null;
  private loadMoreDebounceId: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly options: UseChatScrollOptions) {}

  scrollToBottom(smooth = false): void {
    const list = this.options.messageListRef.value;
    if (!list) return;
    list.scrollTo({
      top: list.scrollHeight,
      behavior: smooth ? "smooth" : "auto",
    });
  }

  attachMessageList(
    element: HTMLElement | null,
    previous: HTMLElement | null,
  ): void {
    previous?.removeEventListener("scroll", this.onScroll);
    this.stopContentObserver();
    if (!element) return;
    this.startContentObserver(element);
    element.addEventListener("scroll", this.onScroll, { passive: true });
  }

  async handleLoadingChange(loading: boolean): Promise<void> {
    if (loading || !this.shouldRecoverAfterLoading()) return;
    await nextTick();
    requestAnimationFrame(() => {
      requestAnimationFrame(() => this.handleContentChange());
    });
  }

  handleMessageCountChange(messageCount: number): void {
    if (messageCount === 0) this.reset();
  }

  checkAndLoadMore(): void {
    this.scheduleAutoLoadMore();
  }

  reset(): void {
    this.scrollScene.value = "initial";
    this.showScrollToBottom.value = false;
    this.loadAnchor = null;
    this.lastScrollHeight = 0;
    this.isInitialRendering.value = true;
    this.cancelScrollRaf();
    this.cancelLoadMoreDebounce();
  }

  dispose(): void {
    this.stopContentObserver();
    if (this.scrollThrottleId !== null) {
      cancelAnimationFrame(this.scrollThrottleId);
      this.scrollThrottleId = null;
    }
    this.cancelLoadMoreDebounce();
    this.options.messageListRef.value?.removeEventListener(
      "scroll",
      this.onScroll,
    );
    this.loadAnchor = null;
  }

  private shouldRecoverAfterLoading(): boolean {
    return (
      this.scrollScene.value === "initial" ||
      (this.scrollScene.value === "loading-top" && this.loadAnchor !== null)
    );
  }

  private prepareLoadAnchor(): void {
    const list = this.options.messageListRef.value;
    if (!list) return;
    const listTop = list.getBoundingClientRect().top;
    const messages = list.querySelectorAll<HTMLElement>("[data-message-id]");
    const visible = Array.from(messages).find(
      (element) => element.getBoundingClientRect().top >= listTop,
    );
    if (!visible) return;
    const messageId = visible.getAttribute("data-message-id");
    if (messageId === null) return;
    this.loadAnchor = {
      messageId,
      offsetFromTop: visible.getBoundingClientRect().top - listTop,
    };
  }

  private restoreScrollByAnchor(): void {
    const list = this.options.messageListRef.value;
    const anchor = this.loadAnchor;
    if (!anchor || !list) return;
    const element = Array.from(
      list.querySelectorAll<HTMLElement>("[data-message-id]"),
    ).find(
      (candidate) =>
        candidate.getAttribute("data-message-id") === anchor.messageId,
    );
    if (element) {
      list.scrollTop = element.offsetTop - anchor.offsetFromTop;
    }
    this.loadAnchor = null;
  }

  private evaluateAutoLoadMore(): void {
    const list = this.options.messageListRef.value;
    if (!list) return;
    this.isInitialRendering.value = false;
    const shouldLoad =
      this.scrollScene.value !== "initial" &&
      list.scrollHeight <= list.clientHeight + 10 &&
      this.options.hasMoreHistory.value &&
      !this.options.isLoadingHistory.value;
    if (!shouldLoad) return;
    this.prepareLoadAnchor();
    this.scrollScene.value = "loading-top";
    this.options.onLoadMore();
  }

  private scheduleAutoLoadMore(): void {
    this.cancelLoadMoreDebounce();
    this.loadMoreDebounceId = setTimeout(() => {
      this.loadMoreDebounceId = null;
      this.evaluateAutoLoadMore();
    }, 200);
  }

  private cancelLoadMoreDebounce(): void {
    if (this.loadMoreDebounceId === null) return;
    clearTimeout(this.loadMoreDebounceId);
    this.loadMoreDebounceId = null;
  }

  private handleContentChange(): void {
    const list = this.options.messageListRef.value;
    if (!list || list.scrollHeight === this.lastScrollHeight) return;
    this.lastScrollHeight = list.scrollHeight;

    if (this.scrollScene.value === "initial") {
      this.handleInitialScene();
      return;
    }
    if (this.handleLoadedPage()) return;
    if (
      this.scrollScene.value === "following" &&
      !this.showScrollToBottom.value
    ) {
      this.scrollToBottom();
    }
    this.scheduleAutoLoadMore();
  }

  private handleInitialScene(): void {
    if (this.options.isLoadingHistory.value) return;
    if (this.options.messageCount.value === 0) {
      this.scrollScene.value = "free";
      this.isInitialRendering.value = false;
      return;
    }
    this.scrollToBottom();
    this.scrollScene.value = "following";
    this.scheduleAutoLoadMore();
  }

  private handleLoadedPage(): boolean {
    if (
      this.scrollScene.value !== "loading-top" ||
      this.options.isLoadingHistory.value
    ) {
      return false;
    }
    if (this.loadAnchor) {
      this.restoreScrollByAnchor();
      this.scrollScene.value = "free";
    } else {
      this.scrollToBottom();
      this.scrollScene.value = "following";
    }
    return true;
  }

  private startContentObserver(element: HTMLElement): void {
    const target =
      element.querySelector(".messages-inner-container") ?? element;
    this.resizeObserver = new ResizeObserver(this.handleResize);
    this.resizeObserver.observe(target);
  }

  private stopContentObserver(): void {
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
    this.cancelScrollRaf();
  }

  private readonly handleResize = (): void => {
    const immediate =
      this.scrollScene.value === "following" ||
      this.scrollScene.value === "loading-top";
    this.cancelScrollRaf();
    if (immediate) {
      this.handleContentChange();
      return;
    }
    this.scrollRafId = requestAnimationFrame(() => {
      this.scrollRafId = null;
      this.handleContentChange();
    });
  };

  private cancelScrollRaf(): void {
    if (this.scrollRafId === null) return;
    cancelAnimationFrame(this.scrollRafId);
    this.scrollRafId = null;
  }

  private readonly onScroll = (): void => {
    if (this.scrollThrottleId !== null) return;
    this.scrollThrottleId = requestAnimationFrame(() => {
      this.scrollThrottleId = null;
      this.processScroll();
    });
  };

  private processScroll(): void {
    const list = this.options.messageListRef.value;
    if (!list) return;
    if (this.isInitialRendering.value) {
      this.showScrollToBottom.value = false;
      this.scrollScene.value = "following";
      return;
    }

    const nearTop = list.scrollTop < 100;
    const nearBottom =
      list.scrollHeight - list.scrollTop - list.clientHeight < 150;
    this.showScrollToBottom.value = !nearBottom;
    this.transitionScrollScene(nearBottom);
    if (nearTop) this.loadMoreFromTop();
  }

  private transitionScrollScene(nearBottom: boolean): void {
    if (nearBottom && this.scrollScene.value === "free") {
      this.scrollScene.value = "following";
    } else if (!nearBottom && this.scrollScene.value === "following") {
      this.scrollScene.value = "free";
    }
  }

  private loadMoreFromTop(): void {
    if (
      this.scrollScene.value !== "free" ||
      !this.options.hasMoreHistory.value ||
      this.options.isLoadingHistory.value
    ) {
      return;
    }
    this.prepareLoadAnchor();
    this.scrollScene.value = "loading-top";
    this.options.onLoadMore();
  }
}
