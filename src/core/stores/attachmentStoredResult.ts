import type { Attachment } from "../types/chat";

export type StoredAttachmentResult = Pick<
  Attachment,
  "type" | "name" | "size" | "src" | "hash" | "thumbnailPath"
>;

function readRequiredString(
  value: Record<string, unknown>,
  key: string,
  label: string,
): string {
  const field = value[key];
  if (typeof field !== "string" || field.trim() === "") {
    throw new Error(`附件服务返回的${label}无效`);
  }
  return field;
}

function readSize(value: Record<string, unknown>): number {
  const size = value.size;
  if (!Number.isSafeInteger(size) || (size as number) < 0) {
    throw new Error("附件服务返回的大小无效");
  }
  return size as number;
}

function readThumbnailPath(value: Record<string, unknown>): string | undefined {
  const path = value.thumbnailPath;
  if (path === null || path === undefined) return undefined;
  if (typeof path !== "string" || path.trim() === "") {
    throw new Error("附件服务返回的缩略图路径无效");
  }
  return path;
}

export function parseStoredAttachmentResult(
  value: unknown,
): StoredAttachmentResult {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("附件服务返回格式错误");
  }
  const record = value as Record<string, unknown>;
  const hash = readRequiredString(record, "hash", "哈希");
  if (!/^[a-f\d]{64}$/i.test(hash)) {
    throw new Error("附件服务返回的哈希无效");
  }
  return {
    type: readRequiredString(record, "type", "类型"),
    name: readRequiredString(record, "name", "名称"),
    size: readSize(record),
    src: readRequiredString(record, "internalPath", "内部路径"),
    hash,
    thumbnailPath: readThumbnailPath(record),
  };
}
