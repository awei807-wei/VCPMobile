import { computed, nextTick, ref } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useChatScroll } from "../../core/composables/useChatScroll";

function setRect(
  element: HTMLElement,
  rect: { top: number; bottom: number },
): void {
  Object.defineProperty(element, "getBoundingClientRect", {
    configurable: true,
    value: () => ({
      top: rect.top,
      bottom: rect.bottom,
      left: 0,
      right: 100,
      width: 100,
      height: rect.bottom - rect.top,
      x: 0,
      y: rect.top,
      toJSON: () => ({}),
    }),
  });
}

describe("useChatScroll history anchors", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("通过属性值查找特殊字符 message id，并在分页后恢复视口锚点", async () => {
    vi.useFakeTimers();
    const originalResizeObserver = globalThis.ResizeObserver;
    let triggerResize: () => void = () => undefined;

    class TestResizeObserver {
      constructor(callback: ResizeObserverCallback) {
        triggerResize = () => callback([], this as unknown as ResizeObserver);
      }

      observe(): void {
        // The composable only needs the observer lifecycle for this test.
      }

      disconnect(): void {
        // The composable only needs the observer lifecycle for this test.
      }
    }

    Object.defineProperty(globalThis, "ResizeObserver", {
      configurable: true,
      writable: true,
      value: TestResizeObserver,
    });

    try {
      const list = document.createElement("div");
      const specialMessageId = 'msg"[]\\:#,?';
      const anchor = document.createElement("div");
      anchor.setAttribute("data-message-id", specialMessageId);
      Object.defineProperty(anchor, "offsetTop", {
        configurable: true,
        value: 120,
      });
      setRect(anchor, { top: 120, bottom: 220 });
      list.append(anchor);

      Object.defineProperties(list, {
        clientHeight: { configurable: true, writable: true, value: 300 },
        scrollHeight: { configurable: true, writable: true, value: 1000 },
      });
      setRect(list, { top: 100, bottom: 400 });
      Object.defineProperty(list, "scrollTo", {
        configurable: true,
        value: vi.fn((options: ScrollToOptions) => {
          if (typeof options.top === "number") list.scrollTop = options.top;
        }),
      });

      const messageListRef = ref<HTMLElement | null>(null);
      const isLoadingHistory = ref(false);
      const onLoadMore = vi.fn(() => {
        isLoadingHistory.value = true;
      });
      const scroll = useChatScroll({
        messageListRef,
        messageCount: computed(() => 1),
        hasMoreHistory: ref(true),
        isLoadingHistory,
        onLoadMore,
      });

      messageListRef.value = list;
      await nextTick();
      triggerResize();
      await vi.runAllTimersAsync();

      list.scrollTop = 0;
      list.dispatchEvent(new Event("scroll"));
      await vi.runAllTimersAsync();
      expect(onLoadMore).toHaveBeenCalledTimes(1);

      const older = document.createElement("div");
      older.setAttribute("data-message-id", "older");
      list.insertBefore(older, anchor);
      Object.defineProperty(list, "scrollHeight", { value: 1100 });
      isLoadingHistory.value = false;
      await nextTick();
      await vi.runAllTimersAsync();

      expect(list.scrollTop).toBe(100);
      scroll.dispose();
    } finally {
      Object.defineProperty(globalThis, "ResizeObserver", {
        configurable: true,
        writable: true,
        value: originalResizeObserver,
      });
    }
  });
});
