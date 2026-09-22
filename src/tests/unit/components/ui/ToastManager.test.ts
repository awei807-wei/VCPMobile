// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, disposePinia, setActivePinia, type Pinia } from "pinia";
import { nextTick } from "vue";
import ToastItem from "../../../../components/ui/ToastItem.vue";
import ToastManager from "../../../../components/ui/ToastManager.vue";
import {
  useNotificationStore,
  type VcpNotification,
} from "../../../../core/stores/notification";

let pinia: Pinia;
let wrapper: VueWrapper | null = null;

const makeToast = (index: number): VcpNotification => ({
  id: `toast-${index}`,
  type: "agent",
  title: `消息 ${index}`,
  message: `第 ${index} 条消息`,
  timestamp: index,
  duration: 0,
});

const renderedToastIds = () =>
  wrapper
    ?.findAllComponents(ToastItem)
    .map((item) => (item.props("toast") as VcpNotification).id) ?? [];

beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  wrapper = mount(ToastManager, {
    global: { plugins: [pinia] },
  });
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
  disposePinia(pinia);
});

describe("ToastManager", () => {
  it("最多两条活动通知直接展示且不提供展开入口", async () => {
    const store = useNotificationStore();
    store.activeToasts = [makeToast(1), makeToast(2)];
    await nextTick();

    expect(renderedToastIds()).toEqual(["toast-1", "toast-2"]);
    expect(
      wrapper?.find('[data-testid="toast-collapse-toggle"]').exists(),
    ).toBe(false);
  });

  it("第三条后端通知只进入通知栏，关闭现有通知后也不会补弹", async () => {
    const store = useNotificationStore();
    [1, 2, 3].forEach((index) => store.addNotification(makeToast(index)));
    await nextTick();

    expect(renderedToastIds()).toEqual(["toast-1", "toast-2"]);
    expect(store.historyList.map((item) => item.id)).toEqual([
      "toast-3",
      "toast-2",
      "toast-1",
    ]);
    expect(store.unreadCount).toBe(3);

    store.activeToasts = store.activeToasts.filter(
      (toast) => toast.id !== "toast-1",
    );
    await nextTick();

    expect(renderedToastIds()).toEqual(["toast-2"]);
    expect(store.activeToasts.some((toast) => toast.id === "toast-3")).toBe(
      false,
    );
  });

  it("通知栏打开时新消息只收纳到历史", async () => {
    const store = useNotificationStore();
    store.isDrawerOpen = true;
    store.addNotification(makeToast(1));
    await nextTick();

    expect(renderedToastIds()).toEqual([]);
    expect(store.historyList.map((item) => item.id)).toEqual(["toast-1"]);
  });
});
