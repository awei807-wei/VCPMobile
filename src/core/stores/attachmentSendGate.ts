import type { Attachment } from "../types/chat";

/** 尚不能安全加入消息的附件状态。 */
export const ATTACHMENT_WORKING_STATUSES = ["loading", "processing"] as const;

export function isAttachmentWorking(
  attachment: Pick<Attachment, "status">,
): boolean {
  return ATTACHMENT_WORKING_STATUSES.includes(
    attachment.status as (typeof ATTACHMENT_WORKING_STATUSES)[number],
  );
}

export function hasWorkingAttachments(
  attachments: readonly Pick<Attachment, "status">[],
): boolean {
  return attachments.some(isAttachmentWorking);
}

/**
 * 任一暂存附件仍在复制、计算哈希或处理时，发送必须保持关闭。
 * 输入框和历史仓库共用此纯函数，避免界面路径与直接调用路径产生分歧。
 */
export function canSendStagedAttachments(
  attachments: readonly Pick<Attachment, "status">[],
): boolean {
  return !hasWorkingAttachments(attachments);
}
