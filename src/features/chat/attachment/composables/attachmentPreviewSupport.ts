import { AttachmentRegistry } from "../AttachmentRegistry";
import { AttachmentType } from "../types/AttachmentType";
import { classifyAttachment } from "../utils/AttachmentClassifier";
import { isDesktopOnlyAttachment } from "../utils/attachmentAvailability";
import type { Component } from "vue";
import type { Attachment } from "../../../../core/types/chat";

export const MAX_PREVIEW_TEXT_LENGTH = 4096;

export function getAttachmentType(attachment: Attachment): AttachmentType {
  return classifyAttachment(attachment.type, attachment.name);
}

export function canPreviewAttachment(attachment: Attachment): boolean {
  if (isDesktopOnlyAttachment(attachment)) return false;
  const type = getAttachmentType(attachment);
  if ([AttachmentType.IMAGE, AttachmentType.VIDEO, AttachmentType.TEXT].includes(type)) {
    return true;
  }
  if (type === AttachmentType.AUDIO && attachment.src) return true;
  if (type === AttachmentType.DOCUMENT && attachment.extractedText) return true;
  if (type === AttachmentType.CODE && attachment.extractedText) return true;
  return false;
}

export function getPreviewComponentFor(attachment: Attachment): Component | null {
  if (isDesktopOnlyAttachment(attachment)) return null;
  return AttachmentRegistry.getComponent(getAttachmentType(attachment));
}

export function formatFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const base = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(
    sizes.length - 1,
    Math.max(0, Math.floor(Math.log(bytes) / Math.log(base))),
  );
  return `${parseFloat((bytes / Math.pow(base, index)).toFixed(1))} ${sizes[index]}`;
}

export function getFileExtension(attachment: Attachment): string {
  return attachment.name.split(".").pop()?.toLowerCase() || "";
}

function safePreviewLength(maxLength: number): number {
  if (Number.isNaN(maxLength) || maxLength === Number.NEGATIVE_INFINITY) return 0;
  if (maxLength === Number.POSITIVE_INFINITY) return MAX_PREVIEW_TEXT_LENGTH;
  return Math.min(MAX_PREVIEW_TEXT_LENGTH, Math.max(0, Math.floor(maxLength)));
}

export function getPreviewText(attachment: Attachment, maxLength = 100): string {
  if (!attachment.extractedText || isDesktopOnlyAttachment(attachment)) return "";
  const length = safePreviewLength(maxLength);
  if (length === 0) return "";
  const text = attachment.extractedText;
  if (text.length <= length) return text;
  const truncated = text.substring(0, length);
  const lastSpace = truncated.lastIndexOf(" ");
  return lastSpace > 0 ? `${truncated.substring(0, lastSpace)}...` : `${truncated}...`;
}

export function hasThumbnail(attachment: Attachment): boolean {
  return !!attachment.thumbnailPath;
}

export interface AttachmentStats {
  total: number;
  byType: Record<AttachmentType, number>;
  canPreview: number;
  hasText: number;
  hasThumbnails: number;
}

export function getAttachmentStats(attachments: Attachment[]): AttachmentStats {
  const stats: AttachmentStats = {
    total: attachments.length,
    byType: {} as Record<AttachmentType, number>,
    canPreview: 0,
    hasText: 0,
    hasThumbnails: 0,
  };
  attachments.forEach((attachment) => {
    const type = getAttachmentType(attachment);
    stats.byType[type] = (stats.byType[type] || 0) + 1;
    if (canPreviewAttachment(attachment)) stats.canPreview++;
    if (attachment.extractedText) stats.hasText++;
    if (hasThumbnail(attachment)) stats.hasThumbnails++;
  });
  return stats;
}
