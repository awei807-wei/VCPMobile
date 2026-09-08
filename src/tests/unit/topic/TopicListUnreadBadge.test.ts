// @vitest-environment happy-dom

import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TopicList from "@/features/topic/TopicList.vue";
import { useTopicStore } from "@/core/stores/topicListManager";

vi.mock("vue-router", () => ({
  useRouter: () => ({
    currentRoute: { value: { path: "/chat" } },
    push: vi.fn(),
  }),
}));

const wrappers: ReturnType<typeof mount>[] = [];

const topic = (unreadCount?: number, unread = false) => ({
  id: "topic-1",
  ownerId: "owner-1",
  ownerType: "agent" as const,
  name: "测试话题",
  createdAt: 1,
  locked: true,
  unread,
  unreadCount,
});

const findUnreadIndicator = (wrapper: ReturnType<typeof mount>) =>
  wrapper.findAll("div").find((element) =>
    element.classes().includes("absolute") &&
    (element.classes().includes("w-3") ||
      element.classes().includes("min-w-[18px]")),
  );

afterEach(() => {
  wrappers.splice(0).forEach((wrapper) => wrapper.unmount());
});

describe("TopicList 未读角标", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it.each([
    { unreadCount: 1, unread: true, label: "1" },
    { unreadCount: 2, unread: true, label: "2" },
    { unreadCount: 100, unread: true, label: "99+" },
  ])(
    "正数 unreadCount=$unreadCount 时优先显示数字 $label",
    async ({ unreadCount, unread, label }) => {
      const store = useTopicStore();
      store.topics = [topic(unreadCount, unread)];
      const wrapper = mount(TopicList);
      wrappers.push(wrapper);

      await nextTick();

      const indicator = findUnreadIndicator(wrapper);
      expect(indicator?.classes()).toContain("min-w-[18px]");
      expect(indicator?.text()).toBe(label);
    },
  );

  it.each([
    { unreadCount: -1, unread: false, label: "未知计数显示红点" },
    { unreadCount: 0, unread: true, label: "零计数但 unread 显示红点" },
    { unreadCount: undefined, unread: true, label: "无计数但 unread 显示红点" },
  ])("$label", async ({ unreadCount, unread }) => {
    const store = useTopicStore();
    store.topics = [topic(unreadCount, unread)];
    const wrapper = mount(TopicList);
    wrappers.push(wrapper);

    await nextTick();

    const indicator = findUnreadIndicator(wrapper);
    expect(indicator?.classes()).toContain("w-3");
    expect(indicator?.text()).toBe("");
  });

  it("零计数且 unread=false 时不显示未读角标", async () => {
    const store = useTopicStore();
    store.topics = [topic(0, false)];
    const wrapper = mount(TopicList);
    wrappers.push(wrapper);

    await nextTick();

    expect(findUnreadIndicator(wrapper)).toBeUndefined();
  });
});
