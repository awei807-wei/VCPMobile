import { describe, expect, it } from "vitest";
import {
  MAX_PREVIEW_TEXT_LENGTH,
  useAttachmentPreview,
} from "@/features/chat/attachment/composables/useAttachmentPreview";
import type { Attachment } from "@/core/types/chat";

function attachment(overrides: Partial<Attachment> = {}): Attachment {
  return {
    type: "image/png",
    name: "sample.png",
    size: 1024,
    src: "file:///sample.png",
    ...overrides,
  };
}

describe("附件预览边界", () => {
  it("按附件类型和桌面端标记决定预览与渲染组件", () => {
    const preview = useAttachmentPreview();
    expect(preview.canPreview.value(attachment())).toBe(true);
    expect(
      preview.canPreview.value(attachment({ type: "text/plain", name: "a.txt" })),
    ).toBe(true);
    expect(
      preview.canPreview.value(
        attachment({ type: "audio/ogg", name: "a.ogg", src: "" }),
      ),
    ).toBe(false);
    expect(
      preview.canPreview.value(
        attachment({ type: "audio/ogg", name: "a.ogg", src: "blob:a" }),
      ),
    ).toBe(true);

    const desktopOnly = attachment({ status: "desktop_only" });
    expect(preview.canPreview.value(desktopOnly)).toBe(false);
    expect(preview.getPreviewComponent.value(desktopOnly)).toBeNull();
  });

  it("文件大小格式化在零值和单位边界保持稳定", () => {
    const preview = useAttachmentPreview();
    expect(preview.formatFileSize(0)).toBe("0 B");
    expect(preview.formatFileSize(-1)).toBe("0 B");
    expect(preview.formatFileSize(Number.NaN)).toBe("0 B");
    expect(preview.formatFileSize(Number.POSITIVE_INFINITY)).toBe("0 B");
    expect(preview.formatFileSize(0.5)).toBe("0.5 B");
    expect(preview.formatFileSize(1024)).toBe("1 KB");
    expect(preview.formatFileSize(1024 * 1024)).toBe("1 MB");
    expect(preview.formatFileSize(1024 ** 4)).toBe("1 TB");
  });

  it("文本预览拒绝负数并限制超大长度参数", () => {
    const preview = useAttachmentPreview();
    const text = "x".repeat(MAX_PREVIEW_TEXT_LENGTH + 32) + "尾部不应被读取";
    const file = attachment({
      type: "text/plain",
      name: "sample.txt",
      extractedText: text,
    });

    expect(preview.getPreviewText.value(file, -1)).toBe("");
    expect(preview.getPreviewText.value(file, 0)).toBe("");
    const bounded = preview.getPreviewText.value(file, Number.MAX_SAFE_INTEGER);
    expect(bounded.length).toBeLessThanOrEqual(MAX_PREVIEW_TEXT_LENGTH + 3);
    expect(bounded).not.toContain("尾部不应被读取");
  });
});
