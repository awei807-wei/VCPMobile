import { computed, reactive, ref } from "vue";
import {
  makeMessageIdentity,
  sameConversationIdentity,
  type ConversationIdentity,
} from "../../core/stores/chatStoreIdentity";
import { tauriSearchAdapter, searchResultKey } from "./searchAdapter";
import {
  createDefaultSearchFilter,
  normalizeSearchFilter,
  SEARCH_DEBOUNCE_MS,
  validateSearchDraft,
} from "./searchValidation";
import type {
  FtsIndexStatus,
  IndexState,
  PendingSearchJump,
  SearchAdapter,
  SearchFilterDraft,
  SearchPage,
  SearchResult,
  SearchStatus,
  TargetStatus,
} from "./types";

interface SearchRequest {
  generation: number;
  append: boolean;
  filter: ReturnType<typeof normalizeSearchFilter>;
}

export class GlobalSearchController {
  readonly isOpen = ref(false);
  readonly filters = reactive<SearchFilterDraft>(createDefaultSearchFilter());
  readonly results = ref<SearchResult[]>([]);
  readonly nextCursor = ref<string | null>(null);
  readonly searchStatus = ref<SearchStatus>("idle");
  readonly validationMessage = ref("");
  readonly indexState = ref<IndexState>("unknown");
  readonly indexStatus = ref<FtsIndexStatus | null>(null);
  readonly targetStatus = ref<TargetStatus>("idle");
  readonly jumpRevision = ref(0);
  readonly activeRequestCount = ref(0);
  readonly hasMore = computed(() => Boolean(this.nextCursor.value));
  readonly isBusy = computed(() => this.searchStatus.value === "loading");

  private readonly adapter: SearchAdapter;
  private generation = 0;
  private requestTimer: ReturnType<typeof setTimeout> | null = null;
  private queuedRequest: SearchRequest | null = null;
  private paginationGeneration: number | null = null;
  private statusRequest: Promise<void> | null = null;
  private pendingJump: PendingSearchJump | null = null;
  private jumpSequence = 0;

  constructor(adapter: SearchAdapter = tauriSearchAdapter) {
    this.adapter = adapter;
  }

  open(): void {
    this.isOpen.value = true;
    void this.refreshIndexStatus();
  }

  close(): void {
    this.isOpen.value = false;
    this.cancelScheduledRequest();
    this.generation += 1;
    this.queuedRequest = null;
    this.paginationGeneration = null;
    if (this.targetStatus.value !== "loading") {
      this.pendingJump = null;
      this.targetStatus.value = "idle";
    }
  }

  dispose(): void {
    this.close();
  }

  setQuery(query: string): void {
    this.filters.query = query;
    this.scheduleSearch();
  }

  setFilter(patch: Partial<SearchFilterDraft>): void {
    const previousOwnerType = this.filters.ownerType;
    Object.assign(this.filters, patch);
    const ownerTypeChanged =
      "ownerType" in patch && patch.ownerType !== previousOwnerType;
    if (
      !this.filters.ownerType ||
      (ownerTypeChanged && !("ownerId" in patch))
    ) {
      this.filters.ownerId = "";
      this.filters.topicId = "";
    }
    this.scheduleSearch();
  }

  setSort(sort: SearchFilterDraft["sort"]): void {
    this.filters.sort = sort;
    this.scheduleSearch();
  }

  loadMore(): void {
    if (!this.nextCursor.value || this.paginationGeneration !== null) return;
    const generation = this.generation;
    const filter = normalizeSearchFilter(this.filters, this.nextCursor.value);
    this.paginationGeneration = generation;
    this.searchStatus.value = "loading";
    this.enqueueRequest({ generation, append: true, filter });
  }

  retry(): void {
    if (!this.filters.query.trim()) return;
    this.cancelScheduledRequest();
    this.queuedRequest = null;
    this.beginFirstPage(this.generation + 1);
  }

  async refreshIndexStatus(): Promise<void> {
    if (this.statusRequest) return this.statusRequest;
    this.indexState.value = "checking";
    this.statusRequest = this.adapter
      .getIndexStatus()
      .then((status) => {
        this.indexStatus.value = status;
        this.indexState.value = deriveIndexState(status);
      })
      .catch(() => {
        this.indexStatus.value = null;
        this.indexState.value = "unavailable";
      })
      .finally(() => {
        this.statusRequest = null;
      });
    return this.statusRequest;
  }

  async rebuildIndex(): Promise<void> {
    if (this.indexState.value === "rebuilding") return;
    this.indexState.value = "rebuilding";
    try {
      await this.adapter.rebuildIndex();
      await this.refreshIndexStatus();
    } catch {
      this.indexState.value = "failed";
    }
  }

  beginJump(result: SearchResult): number | null {
    const identity = makeMessageIdentity(
      result.ownerId,
      result.ownerType,
      result.topicId,
      result.msgId,
    );
    if (!identity) {
      this.targetStatus.value = "invalid";
      return null;
    }
    const requestId = ++this.jumpSequence;
    this.pendingJump = { ...result, requestId };
    this.jumpRevision.value += 1;
    this.targetStatus.value = "loading";
    return requestId;
  }

  takePendingJump(identity: ConversationIdentity): PendingSearchJump | null {
    if (!this.pendingJump) return null;
    const target = makeMessageIdentity(
      this.pendingJump.ownerId,
      this.pendingJump.ownerType,
      this.pendingJump.topicId,
      this.pendingJump.msgId,
    );
    if (!target || !sameConversationIdentity(target, identity)) return null;
    const pending = this.pendingJump;
    this.pendingJump = null;
    return pending;
  }

  resolveJump(
    requestId: number,
    status: Exclude<TargetStatus, "idle" | "loading">,
  ): void {
    if (
      this.pendingJump?.requestId !== undefined &&
      this.pendingJump.requestId !== requestId
    )
      return;
    if (!this.pendingJump && requestId !== this.jumpSequence) return;
    this.targetStatus.value = status;
    if (this.pendingJump?.requestId === requestId) this.pendingJump = null;
  }

  private scheduleSearch(): void {
    this.cancelScheduledRequest();
    this.generation += 1;
    this.queuedRequest = null;
    this.paginationGeneration = null;
    this.results.value = [];
    this.nextCursor.value = null;
    const validation = validateSearchDraft(this.filters);
    this.validationMessage.value = validation ?? "";
    if (validation) {
      this.searchStatus.value = "invalid";
      return;
    }
    if (!this.filters.query.trim()) {
      this.searchStatus.value = "idle";
      return;
    }
    this.searchStatus.value = "loading";
    const generation = this.generation;
    this.requestTimer = setTimeout(() => {
      this.requestTimer = null;
      this.beginFirstPage(generation);
    }, SEARCH_DEBOUNCE_MS);
  }

  private beginFirstPage(generation: number): void {
    this.generation = generation;
    this.results.value = [];
    this.nextCursor.value = null;
    this.paginationGeneration = null;
    this.searchStatus.value = "loading";
    this.enqueueRequest({
      generation,
      append: false,
      filter: normalizeSearchFilter(this.filters),
    });
  }

  private enqueueRequest(request: SearchRequest): void {
    if (this.activeRequestCount.value >= 2) {
      this.queuedRequest = request;
      return;
    }
    this.executeRequest(request);
  }

  private executeRequest(request: SearchRequest): void {
    this.activeRequestCount.value += 1;
    void this.adapter
      .search(request.filter)
      .then((page) => this.applyPage(request, page))
      .catch(() => this.handleRequestError(request))
      .finally(() => this.finishRequest(request));
  }

  private applyPage(request: SearchRequest, page: SearchPage): void {
    if (request.generation !== this.generation) return;
    this.results.value = request.append
      ? mergeResults(this.results.value, page.results)
      : dedupeResults(page.results);
    this.nextCursor.value = page.nextCursor;
    this.searchStatus.value = this.results.value.length > 0 ? "ready" : "empty";
  }

  private handleRequestError(request: SearchRequest): void {
    if (request.generation === this.generation)
      this.searchStatus.value = "error";
  }

  private finishRequest(request: SearchRequest): void {
    this.activeRequestCount.value = Math.max(
      0,
      this.activeRequestCount.value - 1,
    );
    if (this.paginationGeneration === request.generation)
      this.paginationGeneration = null;
    this.drainQueuedRequest();
  }

  private drainQueuedRequest(): void {
    if (this.activeRequestCount.value >= 2 || !this.queuedRequest) return;
    const request = this.queuedRequest;
    this.queuedRequest = null;
    this.executeRequest(request);
  }

  private cancelScheduledRequest(): void {
    if (this.requestTimer === null) return;
    clearTimeout(this.requestTimer);
    this.requestTimer = null;
  }

  expose() {
    return {
      isOpen: this.isOpen,
      filters: this.filters,
      results: this.results,
      nextCursor: this.nextCursor,
      searchStatus: this.searchStatus,
      validationMessage: this.validationMessage,
      indexState: this.indexState,
      indexStatus: this.indexStatus,
      targetStatus: this.targetStatus,
      jumpRevision: this.jumpRevision,
      activeRequestCount: this.activeRequestCount,
      hasMore: this.hasMore,
      isBusy: this.isBusy,
      open: () => this.open(),
      close: () => this.close(),
      dispose: () => this.dispose(),
      setQuery: (query: string) => this.setQuery(query),
      setFilter: (patch: Partial<SearchFilterDraft>) => this.setFilter(patch),
      setSort: (sort: SearchFilterDraft["sort"]) => this.setSort(sort),
      loadMore: () => this.loadMore(),
      retry: () => this.retry(),
      refreshIndexStatus: () => this.refreshIndexStatus(),
      rebuildIndex: () => this.rebuildIndex(),
      beginJump: (result: SearchResult) => this.beginJump(result),
      takePendingJump: (identity: ConversationIdentity) =>
        this.takePendingJump(identity),
      resolveJump: (
        requestId: number,
        status: Exclude<TargetStatus, "idle" | "loading">,
      ) => this.resolveJump(requestId, status),
    };
  }
}

function dedupeResults(results: SearchResult[]): SearchResult[] {
  return mergeResults([], results);
}

function mergeResults(
  current: SearchResult[],
  incoming: SearchResult[],
): SearchResult[] {
  const seen = new Set(current.map(searchResultKey));
  const merged = [...current];
  for (const result of incoming) {
    const key = searchResultKey(result);
    if (seen.has(key)) continue;
    seen.add(key);
    merged.push(result);
  }
  return merged;
}

function deriveIndexState(status: FtsIndexStatus): IndexState {
  if (!status.available || !status.schemaValid || !status.tokenizerValid)
    return "missing";
  if (
    !status.healthy ||
    status.liveCount !== status.indexedCount ||
    status.missingCount ||
    status.orphanCount ||
    status.duplicateCount ||
    status.staleCount ||
    status.decodeErrorCount
  ) {
    return "inconsistent";
  }
  return "healthy";
}
