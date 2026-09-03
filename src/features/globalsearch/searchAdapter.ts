import { invoke } from "@tauri-apps/api/core";
import type { ChatMessage } from "../../core/types/chat";
import { clampUnicode } from "./searchValidation";
import type {
  FtsIndexStatus,
  SearchAdapter,
  SearchJumpInput,
  SearchHistoryWindow,
  SearchPage,
  SearchResult,
  SearchOwnerType,
} from "./types";

const SUMMARY_LIMIT = 240;

export const tauriSearchAdapter: SearchAdapter = {
  search(filter) {
    return invoke<unknown>("search_messages_fts", { filter }).then(
      normalizeSearchPage,
    );
  },

  getIndexStatus() {
    return invoke<unknown>("get_fts_index_status").then(normalizeIndexStatus);
  },

  rebuildIndex() {
    return invoke("rebuild_messages_fts");
  },

  loadHistoryAround(input) {
    return invoke<unknown>("load_chat_history_around", { ...input }).then(
      normalizeHistoryWindow,
    );
  },
};

function normalizeSearchPage(raw: unknown): SearchPage {
  const page = asRecord(raw);
  if (!page || !Array.isArray(page.results))
    throw new Error("搜索响应格式无效");
  const results = page.results.map(normalizeSearchResult);
  if (results.some((result) => result === null))
    throw new Error("搜索结果格式无效");
  return {
    results: results.filter(isSearchResult),
    nextCursor: readOptionalNullableString(page, "nextCursor", "next_cursor"),
  };
}

function normalizeSearchResult(raw: unknown): SearchResult | null {
  const value = asRecord(raw);
  const ownerType = value?.ownerType ?? value?.owner_type;
  if (!isOwnerType(ownerType)) return null;
  const msgId = readString(value?.msgId ?? value?.msg_id);
  const topicId = readString(value?.topicId ?? value?.topic_id);
  const ownerId = readString(value?.ownerId ?? value?.owner_id);
  const role = readString(value?.role);
  const timestamp = readFiniteNumber(value?.timestamp);
  const topicTitle = readRequiredString(
    value?.topicTitle ?? value?.topic_title,
  );
  const snippet = readRequiredString(value?.snippet);
  if (
    !msgId ||
    !topicId ||
    !ownerId ||
    !role ||
    timestamp === null ||
    topicTitle === null ||
    snippet === null
  )
    return null;
  const rawRank = value?.rank;
  if (
    rawRank !== null &&
    rawRank !== undefined &&
    readFiniteNumber(rawRank) === null
  )
    return null;
  return {
    msgId,
    topicId,
    ownerType,
    ownerId,
    role,
    speakerAgentId: readNullableString(
      value?.speakerAgentId ?? value?.speaker_agent_id,
    ),
    timestamp,
    topicTitle,
    snippet: clampUnicode(snippet, SUMMARY_LIMIT),
    rank: readNullableNumber(value?.rank),
  };
}

function isSearchResult(value: SearchResult | null): value is SearchResult {
  return value !== null;
}

function normalizeIndexStatus(raw: unknown): FtsIndexStatus {
  const value = asRecord(raw);
  if (!value) throw new Error("索引状态响应格式无效");
  const status: FtsIndexStatus = {
    available: readRequiredBoolean(value.available, "available"),
    schemaValid: readRequiredBoolean(
      value.schemaValid ?? value.schema_valid,
      "schemaValid",
    ),
    tokenizerValid: readRequiredBoolean(
      value.tokenizerValid ?? value.tokenizer_valid,
      "tokenizerValid",
    ),
    liveCount: readRequiredCount(
      value.liveCount ?? value.live_count,
      "liveCount",
    ),
    indexedCount: readRequiredCount(
      value.indexedCount ?? value.indexed_count,
      "indexedCount",
    ),
    missingCount: readRequiredCount(
      value.missingCount ?? value.missing_count,
      "missingCount",
    ),
    orphanCount: readRequiredCount(
      value.orphanCount ?? value.orphan_count,
      "orphanCount",
    ),
    duplicateCount: readRequiredCount(
      value.duplicateCount ?? value.duplicate_count,
      "duplicateCount",
    ),
    staleCount: readRequiredCount(
      value.staleCount ?? value.stale_count,
      "staleCount",
    ),
    decodeErrorCount: readRequiredCount(
      value.decodeErrorCount ?? value.decode_error_count,
      "decodeErrorCount",
    ),
    healthy: readRequiredBoolean(value.healthy, "healthy"),
    diagnostic: readOptionalNullableString(value, "diagnostic"),
  };
  return status;
}

function normalizeHistoryWindow(raw: unknown): SearchHistoryWindow {
  const value = asRecord(raw);
  if (!value || !Array.isArray(value.messages))
    throw new Error("锚点历史响应格式无效");
  const messages = value.messages.map(normalizeChatMessage);
  if (messages.some((message) => message === null))
    throw new Error("锚点历史消息格式无效");
  const nextOffset = readRequiredCount(
    value.nextOffset ?? value.next_offset,
    "nextOffset",
  );
  if (nextOffset < messages.length) throw new Error("锚点历史偏移量无效");
  return {
    messages: messages.filter(
      (message): message is ChatMessage => message !== null,
    ),
    nextOffset,
    hasMoreHistory: readRequiredBoolean(
      value.hasMoreHistory ?? value.has_more_history,
      "hasMoreHistory",
    ),
  };
}

function normalizeChatMessage(raw: unknown): ChatMessage | null {
  const value = asRecord(raw);
  const id = readString(value?.id);
  const role = readString(value?.role);
  const timestamp = readFiniteNumber(value?.timestamp);
  if (!value || !id || !role || timestamp === null || timestamp < 0)
    return null;
  return value as unknown as ChatMessage;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object"
    ? (value as Record<string, unknown>)
    : null;
}

function readString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function readNullableString(value: unknown): string | null {
  const result = readString(value);
  return result || null;
}

function readRequiredString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function readOptionalNullableString(
  value: Record<string, unknown>,
  ...keys: string[]
): string | null {
  const present = keys.find((key) =>
    Object.prototype.hasOwnProperty.call(value, key),
  );
  if (!present || value[present] === null) return null;
  if (typeof value[present] !== "string")
    throw new Error(`${present} 字段格式无效`);
  return value[present] as string;
}

function readFiniteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function readNullableNumber(value: unknown): number | null {
  return readFiniteNumber(value);
}

function readRequiredCount(value: unknown, field: string): number {
  const number = readFiniteNumber(value);
  if (number === null || number < 0 || !Number.isSafeInteger(number))
    throw new Error(`${field} 字段格式无效`);
  return number;
}

function readRequiredBoolean(value: unknown, field: string): boolean {
  if (typeof value !== "boolean") throw new Error(`${field} 字段格式无效`);
  return value;
}

function isOwnerType(value: unknown): value is SearchOwnerType {
  return value === "agent" || value === "group";
}

export function searchResultKey(
  result: Pick<SearchResult, "ownerType" | "ownerId" | "topicId" | "msgId">,
): string {
  return [result.ownerType, result.ownerId, result.topicId, result.msgId]
    .map((part) => encodeURIComponent(part))
    .join(":");
}

export function isValidSearchJump(input: SearchJumpInput): boolean {
  return Boolean(
    input.ownerId &&
    (input.ownerType === "agent" || input.ownerType === "group") &&
    input.topicId &&
    input.anchorMessageId,
  );
}
