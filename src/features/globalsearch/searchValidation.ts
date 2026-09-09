import type { SearchFilter, SearchFilterDraft, SearchOwnerType } from "./types";

export const SEARCH_MAX_CHARS = 128;
export const SEARCH_MAX_BYTES = 512;
export const SEARCH_DEFAULT_LIMIT = 50;
export const SEARCH_MAX_LIMIT = 100;
export const SEARCH_DEBOUNCE_MS = 220;

export function countUtf8Bytes(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}

export function validateSearchQuery(query: string): string | null {
  if (Array.from(query).length > SEARCH_MAX_CHARS) {
    return `搜索词不能超过 ${SEARCH_MAX_CHARS} 个字符`;
  }
  if (countUtf8Bytes(query) > SEARCH_MAX_BYTES) {
    return `搜索词不能超过 ${SEARCH_MAX_BYTES} 字节`;
  }
  if (
    Array.from(query).some(
      (character) => character === "\0" || /\p{Cc}/u.test(character),
    )
  ) {
    return "搜索词包含不支持的控制字符";
  }
  return null;
}

export function validateSearchDraft(draft: SearchFilterDraft): string | null {
  const queryError = validateSearchQuery(draft.query);
  if (queryError) return queryError;
  if (
    draft.startTime !== null &&
    draft.endTime !== null &&
    draft.startTime > draft.endTime
  ) {
    return "起始日期不能晚于结束日期";
  }
  if (draft.ownerId && !draft.ownerType)
    return "所有者 ID 需要先选择所有者类型";
  if (draft.topicId && (!draft.ownerType || !draft.ownerId)) {
    return "话题 ID 需要完整的所有者身份";
  }
  return null;
}

export function normalizeSearchFilter(
  draft: SearchFilterDraft,
  cursor?: string,
): SearchFilter {
  const filter: SearchFilter = {
    query: draft.query.trim(),
    limit: Math.min(
      SEARCH_MAX_LIMIT,
      Math.max(1, Math.trunc(draft.limit || SEARCH_DEFAULT_LIMIT)),
    ),
    sort: draft.sort,
  };
  addOptional(filter, "topicId", draft.topicId.trim() || undefined);
  addOptional(filter, "ownerType", normalizeOwnerType(draft.ownerType));
  addOptional(filter, "ownerId", draft.ownerId.trim() || undefined);
  addOptional(
    filter,
    "speakerAgentId",
    draft.speakerAgentId.trim() || undefined,
  );
  addOptional(filter, "role", draft.role.trim() || undefined);
  addOptional(filter, "startTime", draft.startTime ?? undefined);
  addOptional(filter, "endTime", draft.endTime ?? undefined);
  addOptional(filter, "cursor", cursor);
  return filter;
}

function addOptional<T extends keyof SearchFilter>(
  filter: SearchFilter,
  key: T,
  value: SearchFilter[T] | undefined,
): void {
  if (value !== undefined && value !== "") filter[key] = value;
}

function normalizeOwnerType(
  value: SearchOwnerType | "",
): SearchOwnerType | undefined {
  return value === "agent" || value === "group" ? value : undefined;
}

export function createDefaultSearchFilter(): SearchFilterDraft {
  return {
    query: "",
    topicId: "",
    ownerType: "",
    ownerId: "",
    speakerAgentId: "",
    role: "",
    startTime: null,
    endTime: null,
    limit: SEARCH_DEFAULT_LIMIT,
    sort: "time",
  };
}

export function clampUnicode(value: string, limit: number): string {
  return Array.from(value).slice(0, limit).join("");
}
