import type { ChatMessage } from "../../core/types/chat";

export type SearchSort = "time" | "rank";
export type SearchOwnerType = "agent" | "group";

export interface SearchFilter {
  query: string;
  topicId?: string;
  ownerType?: SearchOwnerType;
  ownerId?: string;
  speakerAgentId?: string;
  role?: string;
  startTime?: number;
  endTime?: number;
  limit?: number;
  sort?: SearchSort;
  cursor?: string;
}

export interface SearchFilterDraft {
  query: string;
  topicId: string;
  ownerType: SearchOwnerType | "";
  ownerId: string;
  speakerAgentId: string;
  role: string;
  startTime: number | null;
  endTime: number | null;
  limit: number;
  sort: SearchSort;
}

export interface SearchResult {
  msgId: string;
  topicId: string;
  ownerType: SearchOwnerType;
  ownerId: string;
  role: string;
  speakerAgentId: string | null;
  timestamp: number;
  topicTitle: string;
  snippet: string;
  rank: number | null;
}

export interface SearchPage {
  results: SearchResult[];
  nextCursor: string | null;
}

export interface FtsIndexStatus {
  available: boolean;
  schemaValid: boolean;
  tokenizerValid: boolean;
  liveCount: number;
  indexedCount: number;
  missingCount: number;
  orphanCount: number;
  duplicateCount: number;
  staleCount: number;
  decodeErrorCount: number;
  healthy: boolean;
  diagnostic: string | null;
}

export type IndexState =
  | "unknown"
  | "checking"
  | "healthy"
  | "missing"
  | "inconsistent"
  | "rebuilding"
  | "failed"
  | "unavailable";

export type SearchStatus =
  | "idle"
  | "loading"
  | "ready"
  | "empty"
  | "invalid"
  | "error";
export type TargetStatus = "idle" | "loading" | "success" | "invalid" | "error";

export interface PendingSearchJump extends SearchResult {
  requestId: number;
}

export interface SearchAdapter {
  search(filter: SearchFilter): Promise<SearchPage>;
  getIndexStatus(): Promise<FtsIndexStatus>;
  rebuildIndex(): Promise<unknown>;
  loadHistoryAround(input: SearchJumpInput): Promise<SearchHistoryWindow>;
}

export interface SearchJumpInput {
  ownerId: string;
  ownerType: SearchOwnerType;
  topicId: string;
  anchorMessageId: string;
  beforeCount?: number;
  afterCount?: number;
}

export interface SearchHistoryWindow {
  messages: ChatMessage[];
  nextOffset: number;
  hasMoreHistory: boolean;
}
