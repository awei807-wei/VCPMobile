import { nextTick } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import AttachmentViewer from "../../features/chat/attachment/AttachmentViewer.vue";

vi.mock("../../core/composables/useModalHistory", () => ({
  useModalHistory: () => ({
    registerModal: vi.fn(),
    unregisterModal: vi.fn(),
  }),
}));

afterEach(() => vi.unstubAllGlobals());

describe("附件查看器仅桌面可用状态", () => {
  it("移动端不读取文件且不提供外部打开", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const wrapper = mount(AttachmentViewer, {
      props: {
        file: {
          type: "application/octet-stream",
          name: "desktop.bin",
          size: 10,
          src: "/desktop.bin",
          status: "desktop_only",
        },
        isOpen: false,
      },
    });

    await wrapper.setProps({ isOpen: true });
    await nextTick();
    expect(wrapper.text()).toContain("仅支持桌面端打开");
    expect(fetchMock).not.toHaveBeenCalled();
    expect(wrapper.findAll("button")).toHaveLength(1);
    wrapper.unmount();
  });

  it("限制单个超大文本块并取消剩余读取流", async () => {
    const cancel = vi.fn().mockResolvedValue(undefined);
    const read = vi.fn().mockResolvedValueOnce({
      done: false,
      value: new TextEncoder().encode(`${"a".repeat(128 * 1024)}TAIL`),
    });
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        body: { getReader: () => ({ read, cancel }) },
      }),
    );
    const wrapper = mount(AttachmentViewer, {
      props: {
        file: {
          type: "text/plain",
          name: "large.txt",
          size: 256 * 1024,
          src: "blob:large-preview",
        },
        isOpen: false,
      },
    });

    await wrapper.setProps({ isOpen: true });
    await flushPromises();
    expect(cancel).toHaveBeenCalledOnce();
    expect(wrapper.text()).toContain("128KB");
    expect(wrapper.text()).not.toContain("TAIL");
    wrapper.unmount();
  });
});
