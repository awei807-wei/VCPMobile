// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import { createPinia, disposePinia, setActivePinia, type Pinia } from "pinia";
import ToolBlock from "../../../../features/chat/blocks/ToolBlock.vue";
import type { ContentBlock } from "../../../../core/types/chat";

let pinia: Pinia;
let wrapper: VueWrapper | null = null;

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

const resultBlock = (status: string): ContentBlock => ({
  type: "tool-result",
  tool_name: "WebSearch",
  status,
  details: [{ key: "返回内容", value: "结果" }],
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

describe("ToolBlock 工具卡状态语义色", () => {
  it.each([
    ["✅ 成功", "status-success"],
    ["❌ 失败", "status-failure"],
    ["⏳ 超时", "status-timeout"],
    ["⛔ 被拒绝", "status-rejected"],
    ["Unknown Status", ""],
  ])("状态「%s」映射为 %s", (status, expected) => {
    wrapper = mount(ToolBlock, {
      props: { type: "tool-result", block: resultBlock(status) },
    });

    const statusEl = wrapper.find(".tool-status");
    expect(statusEl.exists()).toBe(true);
    if (expected) {
      expect(statusEl.classes()).toContain(expected);
    } else {
      expect(
        statusEl.classes().some((name) => name.startsWith("status-")),
      ).toBe(false);
    }
  });
});
