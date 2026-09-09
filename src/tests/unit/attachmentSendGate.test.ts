import { describe, expect, it } from "vitest";
import {
  canSendStagedAttachments,
  hasWorkingAttachments,
  isAttachmentWorking,
} from "../../core/stores/attachmentSendGate";

describe("附件发送门禁", () => {
  it("阻止发送加载中和处理中附件", () => {
    expect(isAttachmentWorking({ status: "loading" })).toBe(true);
    expect(isAttachmentWorking({ status: "processing" })).toBe(true);
    expect(isAttachmentWorking({ status: "done" })).toBe(false);
    expect(
      hasWorkingAttachments([{ status: "done" }, { status: "processing" }]),
    ).toBe(true);
    expect(canSendStagedAttachments([{ status: "done" }])).toBe(true);
  });

  it("允许发送未知状态或仅桌面可用附件", () => {
    expect(canSendStagedAttachments([{ status: "desktop_only" }, {}])).toBe(
      true,
    );
  });
});
