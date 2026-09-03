import type { Attachment } from "../../../../core/types/chat";

export function isDesktopOnlyAttachment(
  attachment: Pick<Attachment, "status"> | null | undefined,
): boolean {
  return attachment?.status === "desktop_only";
}

export function canUseLocalAttachment(
  attachment: Pick<Attachment, "status"> | null | undefined,
): boolean {
  return !isDesktopOnlyAttachment(attachment);
}
