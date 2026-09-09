import { describe, expect, it } from "vitest";
import {
  canUseLocalAttachment,
  isDesktopOnlyAttachment,
} from "../../features/chat/attachment/utils/attachmentAvailability";

describe("附件可用性", () => {
  it("将仅桌面可用附件标记为本地不可用", () => {
    const attachment = { status: "desktop_only" } as const;
    expect(isDesktopOnlyAttachment(attachment)).toBe(true);
    expect(canUseLocalAttachment(attachment)).toBe(false);
    expect(canUseLocalAttachment({ status: "done" })).toBe(true);
  });
});
