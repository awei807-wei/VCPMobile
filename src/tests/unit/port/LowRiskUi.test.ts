// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";
import { createTestingPinia } from "@pinia/testing";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { defineComponent, h, nextTick } from "vue";
import StagedAttachmentPreview from "../../../features/chat/StagedAttachmentPreview.vue";
import GroupMembersSection from "../../../features/agent/GroupMembersSection.vue";
import RagObserver from "../../../features/rag/RagObserver.vue";
import { useRagObserverStore } from "../../../core/stores/ragObserver";
import type { Attachment } from "../../../core/types/chat";

vi.mock("../../../core/stores/theme", () => ({
  useThemeStore: () => ({ isDarkResolved: false }),
}));

const wrappers: VueWrapper[] = [];

afterEach(() => {
  wrappers.splice(0).forEach((wrapper) => wrapper.unmount());
});

function attachment(overrides: Partial<Attachment> = {}): Attachment {
  return {
    type: "text/plain",
    name: "notes.txt",
    size: 12,
    src: "",
    ...overrides,
  };
}

function mountRagObserver() {
  const pinia = createTestingPinia({ stubActions: true });
  const ragStore = useRagObserverStore(pinia);
  ragStore.metadataList = [
    {
      id: "rag-item",
      type: "",
      title: "检索记录",
      subtitle: "测试",
      summary: "可折叠的检索摘要",
      timestamp: new Date(0).toISOString(),
      hasDetails: true,
    },
  ];
  ragStore.fetchPayload = vi.fn().mockResolvedValue({
    query: "这是一段超过三十个字符的检索问题，用于测试收起定位。",
    coreTags: [],
    results: [],
  });

  const wrapper = mount(RagObserver, {
    props: { isOpen: false },
    global: {
      plugins: [pinia],
      stubs: {
        RagPayloadDetail: defineComponent({
          props: { text: { type: String, default: "" } },
          setup(props) {
            return () => h("span", props.text);
          },
        }),
      },
    },
  });
  wrappers.push(wrapper);
  return { wrapper, ragStore };
}

describe("T02 低风险界面修复", () => {
  it("附件删除事件仍携带原 index", async () => {
    const wrapper = mount(StagedAttachmentPreview, {
      props: { file: attachment(), index: 7 },
      global: {
        stubs: {
          AttachmentRenderer: defineComponent({
            emits: ["remove"],
            setup(_, { emit }) {
              return () =>
                h("button", { onClick: () => emit("remove", 999) }, "删除");
            },
          }),
        },
      },
    });
    wrappers.push(wrapper);

    await wrapper.get("button").trigger("click");

    expect(wrapper.emitted("remove")).toEqual([[7]]);
  });

  it("群成员 toggle 事件仍携带原 agentId，并保留滚动限高 class", async () => {
    const wrapper = mount(GroupMembersSection, {
      props: {
        agents: [{ id: "agent-7", name: "测试助手" }],
        members: [],
        memberTags: {},
      },
      global: { plugins: [createTestingPinia({ stubActions: true })] },
    });
    wrappers.push(wrapper);

    expect(wrapper.get(".card-modern").classes()).toEqual(
      expect.arrayContaining(["max-h-80", "overflow-y-auto", "vcp-scrollable"]),
    );
    await wrapper.get('input[type="checkbox"]').setValue(true);

    expect(wrapper.emitted("toggle-member")).toEqual([["agent-7"]]);
  });

  it("RAG 卡片和子卡片收起立即定位，Tab 切换仍平滑定位", async () => {
    const { wrapper } = mountRagObserver();
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });

    const cardHeader = wrapper.get(".vcp-info-card-item > div");
    await cardHeader.trigger("click");
    await flushPromises();

    const queryCard = wrapper
      .findAll("div")
      .find(
        (node) =>
          node.classes().includes("cursor-pointer") &&
          node.text().includes("RAG 检索提问"),
      );
    expect(queryCard).toBeDefined();
    await queryCard!.trigger("click");
    await nextTick();
    await queryCard!.trigger("click");
    await flushPromises();

    expect(scrollIntoView).toHaveBeenCalledWith({
      behavior: "instant",
      block: "nearest",
    });

    await cardHeader.trigger("click");
    await flushPromises();

    expect(scrollIntoView).toHaveBeenCalledWith({
      behavior: "instant",
      block: "nearest",
    });

    scrollIntoView.mockClear();
    await wrapper.get('[data-tab-value="rag"]').trigger("click");
    await flushPromises();

    expect(scrollIntoView).toHaveBeenCalledWith({
      behavior: "smooth",
      block: "nearest",
      inline: "center",
    });
  });
});
