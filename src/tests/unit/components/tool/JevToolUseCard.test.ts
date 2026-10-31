// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, disposePinia, setActivePinia, type Pinia } from "pinia";
import ToolBlock from "../../../../features/chat/blocks/ToolBlock.vue";
import type { ContentBlock } from "../../../../core/types/chat";

let pinia: Pinia;
let wrapper: VueWrapper | null = null;

/** happy-dom 没有 IntersectionObserver，ToolBlock 挂载时只做动画暂停判定。 */
class IntersectionObserverStub {
  root = null;
  rootMargin = "";
  thresholds: number[] = [];
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
}

beforeEach(() => {
  pinia = createPinia();
  setActivePinia(pinia);
  vi.stubGlobal("IntersectionObserver", IntersectionObserverStub);
});

afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
  vi.unstubAllGlobals();
  disposePinia(pinia);
});

describe("ToolBlock JEVToolUse 卡片", () => {
  it("JEV 请求显示 JEVToolUse 标签与解析出的展示名", () => {
    wrapper = mount(ToolBlock, {
      props: {
        type: "tool-use",
        content: "JEV:「始」联络 'GPT生图'「末」",
        block: {
          type: "tool-use",
          tool_name: "Processing...",
          content: "JEV:「始」联络 'GPT生图'「末」",
          is_jev: true,
          jev_display_name: "GPT生图",
        },
      },
    });

    expect(wrapper.find(".tool-label").text()).toContain("JEVToolUse");
    expect(wrapper.find(".tool-name").text()).toBe("GPT生图");
    expect(wrapper.find(".vcp-tool-block").classes()).toContain("is-jev");
  });

  it("JEV 没有展示名时只保留标签，不回退成 Unknown Tool", () => {
    wrapper = mount(ToolBlock, {
      props: {
        type: "tool-use",
        content: "JEV:「始」联络[Grok]，问他今天天气「末」",
        block: {
          type: "tool-use",
          tool_name: "Processing...",
          content: "JEV:「始」联络[Grok]，问他今天天气「末」",
          is_jev: true,
          jev_display_name: "",
        },
      },
    });

    expect(wrapper.find(".tool-label").text()).toContain("JEVToolUse");
    expect(wrapper.find(".tool-name").exists()).toBe(false);
    expect(wrapper.text()).not.toContain("Unknown Tool");
  });

  it("普通工具请求仍是 VCP-ToolUse，不带 JEV 外观", () => {
    wrapper = mount(ToolBlock, {
      props: {
        type: "tool-use",
        content: "tool_name:「始」SciCalculator「末」",
        block: {
          type: "tool-use",
          tool_name: "SciCalculator",
          content: "tool_name:「始」SciCalculator「末」",
        },
      },
    });

    expect(wrapper.find(".tool-label").text()).toContain("VCP-ToolUse");
    expect(wrapper.find(".tool-name").text()).toBe("SciCalculator");
    expect(wrapper.find(".vcp-tool-block").classes()).not.toContain("is-jev");
  });

  it("工具结果块即使带 is_jev 标记也不会套用 JEV 卡片", () => {
    const resultBlock: ContentBlock = {
      type: "tool-result",
      tool_name: "SciCalculator",
      status: "✅ 成功",
      details: [{ key: "返回内容", value: "2" }],
      is_jev: true,
    };
    wrapper = mount(ToolBlock, {
      props: { type: "tool-result", block: resultBlock },
    });

    expect(wrapper.find(".vcp-tool-block").classes()).not.toContain("is-jev");
    expect(wrapper.find(".tool-label").text()).toContain("VCP-ToolResult");
  });
});
