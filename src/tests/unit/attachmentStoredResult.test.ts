import { describe, expect, it } from "vitest";
import { parseStoredAttachmentResult } from "../../core/stores/attachmentStoredResult";

const VALID_HASH = "a".repeat(64);

describe("附件存储结果", () => {
  it("接受 Rust 的 camelCase 响应契约", () => {
    expect(
      parseStoredAttachmentResult({
        type: "text/plain",
        name: "note.txt",
        size: 4,
        internalPath: "/attachments/note.txt",
        hash: VALID_HASH,
        thumbnailPath: "/attachments/note.thumb.png",
      }),
    ).toEqual({
      type: "text/plain",
      name: "note.txt",
      size: 4,
      src: "/attachments/note.txt",
      hash: VALID_HASH,
      thumbnailPath: "/attachments/note.thumb.png",
    });
  });

  it.each([
    ["空响应", null],
    [
      "缺少内部路径",
      { type: "text/plain", name: "a", size: 1, hash: VALID_HASH },
    ],
    [
      "非法哈希",
      {
        type: "text/plain",
        name: "a",
        size: 1,
        internalPath: "/a",
        hash: "bad",
      },
    ],
    [
      "非法大小",
      {
        type: "text/plain",
        name: "a",
        size: -1,
        internalPath: "/a",
        hash: VALID_HASH,
      },
    ],
  ])("拒绝%s", (_label, value) => {
    expect(() => parseStoredAttachmentResult(value)).toThrow();
  });
});
