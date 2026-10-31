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

const toolResult = (hidden: boolean): ContentBlock => ({
  type: "tool-result",
  tool_name: "WebSearch",
  status: "✅ 成功",
  details: [{ key: "返回内容", value: "结果" }],
  hidden,
  raw_hash: "0123456789abcdef",
});

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

describe("ToolBlock 插件回调可见性开关", () => {
  it("默认态显示“已注入上下文”的眼睛按钮", () => {
    wrapper = mount(ToolBlock, {
      props: { type: "tool-result", block: toolResult(false) },
    });

    const button = wrapper.find(".tool-visibility-btn");
    expect(button.exists()).toBe(true);
    expect(button.attributes("title")).toBe(
      "当前已注入上下文（点击在上下文中隐藏）",
    );
    expect(button.attributes("aria-pressed")).toBe("false");
    expect(wrapper.find(".tool-hidden-badge").exists()).toBe(false);
    expect(wrapper.classes()).not.toContain("is-hidden");
  });

  it("隐藏态显示“已隐藏”徽章与恢复提示", () => {
    wrapper = mount(ToolBlock, {
      props: { type: "tool-result", block: toolResult(true) },
    });

    expect(wrapper.find(".tool-hidden-badge").text()).toBe("已隐藏");
    expect(wrapper.find(".tool-visibility-btn").attributes("title")).toBe(
      "当前已从上下文中隐藏（点击恢复注入）",
    );
    expect(
      wrapper.find(".tool-visibility-btn").attributes("aria-pressed"),
    ).toBe("true");
    expect(wrapper.find(".vcp-tool-block").classes()).toContain("is-hidden");
  });

  it("点击眼睛按钮只抛出 toggle-hidden，不触发展开", async () => {
    wrapper = mount(ToolBlock, {
      props: { type: "tool-result", block: toolResult(false) },
    });

    await wrapper.find(".tool-visibility-btn").trigger("click");

    expect(wrapper.emitted("toggle-hidden")).toHaveLength(1);
  });

  it("生成中的消息只能提示、由上层拦截", () => {
    wrapper = mount(ToolBlock, {
      props: { type: "tool-result", block: toolResult(false), streaming: true },
    });

    expect(wrapper.find(".tool-visibility-btn").attributes("title")).toBe(
      "消息生成中，暂时无法修改工具结果状态",
    );
  });

  it("tool-use 块不渲染可见性开关", () => {
    wrapper = mount(ToolBlock, {
      props: {
        type: "tool-use",
        content: "调用参数",
        block: {
          type: "tool-use",
          tool_name: "WebSearch",
          content: "调用参数",
        },
      },
    });

    expect(wrapper.find(".tool-visibility-btn").exists()).toBe(false);
  });
});
