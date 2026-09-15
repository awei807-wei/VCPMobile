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
  message: `第 ${index} 条积压消息`,
  timestamp: index,
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
  it("少量通知直接展示且不渲染收起控制", async () => {
    const store = useNotificationStore();
    store.activeToasts = [makeToast(1), makeToast(2)];
    await nextTick();

    expect(renderedToastIds()).toEqual(["toast-1", "toast-2"]);
    expect(
      wrapper?.find('[data-testid="toast-collapse-toggle"]').exists(),
    ).toBe(false);
  });

  it("积压通知默认只展示最近两条并标明已收起数量", async () => {
    const store = useNotificationStore();
    store.activeToasts = Array.from({ length: 5 }, (_, index) =>
      makeToast(index + 1),
    );
    await nextTick();

    expect(renderedToastIds()).toEqual(["toast-4", "toast-5"]);
    expect(
      wrapper?.get('[data-testid="toast-collapse-toggle"]').text(),
    ).toContain("已收起 3 条通知");
    expect(
      wrapper
        ?.get('[data-testid="toast-collapse-toggle"]')
        .attributes("aria-expanded"),
    ).toBe("false");
    expect(
      wrapper
        ?.findAllComponents(ToastItem)
        .every((item) => item.props("compact") === true),
    ).toBe(true);
    expect(wrapper?.findAll(".vcp-toast-message.is-compact")).toHaveLength(2);
  });

  it("支持展开全部通知并再次收起", async () => {
    const store = useNotificationStore();
    store.activeToasts = Array.from({ length: 4 }, (_, index) =>
      makeToast(index + 1),
    );
    await nextTick();

    const toggle = wrapper?.get('[data-testid="toast-collapse-toggle"]');
    await toggle?.trigger("click");

    expect(renderedToastIds()).toEqual([
      "toast-1",
      "toast-2",
      "toast-3",
      "toast-4",
    ]);
    expect(toggle?.attributes("aria-expanded")).toBe("true");
    expect(
      wrapper
        ?.findAllComponents(ToastItem)
        .every((item) => item.props("compact") === false),
    ).toBe(true);
    expect(wrapper?.findAll(".vcp-toast-message.is-compact")).toHaveLength(0);

    await toggle?.trigger("click");
    expect(renderedToastIds()).toEqual(["toast-3", "toast-4"]);
    expect(toggle?.attributes("aria-expanded")).toBe("false");
  });

  it("通知数量降到阈值后重置展开状态", async () => {
    const store = useNotificationStore();
    store.activeToasts = Array.from({ length: 4 }, (_, index) =>
      makeToast(index + 1),
    );
    await nextTick();
    await wrapper
      ?.get('[data-testid="toast-collapse-toggle"]')
      .trigger("click");

    store.activeToasts = [makeToast(5), makeToast(6)];
    await nextTick();
    store.activeToasts = [makeToast(5), makeToast(6), makeToast(7)];
    await nextTick();

    expect(renderedToastIds()).toEqual(["toast-6", "toast-7"]);
    expect(
      wrapper
        ?.get('[data-testid="toast-collapse-toggle"]')
        .attributes("aria-expanded"),
    ).toBe("false");
  });
});
