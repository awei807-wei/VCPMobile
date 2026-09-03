import { afterEach, describe, expect, it, vi } from "vitest";
import { GlobalSearchController } from "../../../features/globalsearch/searchController";
import { SEARCH_DEBOUNCE_MS } from "../../../features/globalsearch/searchValidation";
import type {
  FtsIndexStatus,
  SearchAdapter,
  SearchFilter,
  SearchPage,
} from "../../../features/globalsearch/types";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function result(id: string) {
  return {
    msgId: id,
    topicId: "topic:with:special[chars]",
    ownerType: "agent" as const,
    ownerId: "owner-a",
    role: "user",
    speakerAgentId: null,
    timestamp: 100,
    topicTitle: "测试话题",
    snippet: id,
    rank: null,
  };
}

function status(overrides: Partial<FtsIndexStatus> = {}): FtsIndexStatus {
  return {
    available: true,
    schemaValid: true,
    tokenizerValid: true,
    liveCount: 2,
    indexedCount: 2,
    missingCount: 0,
    orphanCount: 0,
    duplicateCount: 0,
    staleCount: 0,
    decodeErrorCount: 0,
    healthy: true,
    diagnostic: null,
    ...overrides,
  };
}

function adapterFor(search: SearchAdapter["search"]): SearchAdapter {
  return {
    search,
    getIndexStatus: vi.fn(async () => status()),
    rebuildIndex: vi.fn(async () => undefined),
    loadHistoryAround: vi.fn(async () => ({
      messages: [],
      nextOffset: 0,
      hasMoreHistory: false,
    })),
  };
}

afterEach(() => {
  vi.useRealTimers();
});

describe("全局搜索控制器", () => {
  it("对输入防抖并允许单字符搜索", async () => {
    vi.useFakeTimers();
    const search = vi.fn(
      async (_filter: SearchFilter): Promise<SearchPage> => ({
        results: [result("one")],
        nextCursor: null,
      }),
    );
    const controller = new GlobalSearchController(adapterFor(search));

    controller.setQuery("一");
    await vi.advanceTimersByTimeAsync(219);
    expect(search).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    await vi.waitFor(() => expect(search).toHaveBeenCalledTimes(1));
    expect(search.mock.calls[0]?.[0].query).toBe("一");
    expect(controller.results.value).toHaveLength(1);
  });

  it("仅应用最新代际且并发请求不超过两个", async () => {
    vi.useFakeTimers();
    const requests: ReturnType<typeof deferred<SearchPage>>[] = [];
    const search = vi.fn((_filter: SearchFilter) => {
      const request = deferred<SearchPage>();
      requests.push(request);
      return request.promise;
    });
    const controller = new GlobalSearchController(adapterFor(search));

    controller.setQuery("a");
    await vi.advanceTimersByTimeAsync(220);
    controller.setQuery("b");
    await vi.advanceTimersByTimeAsync(220);
    controller.setQuery("c");
    await vi.advanceTimersByTimeAsync(220);
    expect(controller.activeRequestCount.value).toBe(2);
    expect(search).toHaveBeenCalledTimes(2);

    requests[0]?.resolve({ results: [result("old")], nextCursor: null });
    await vi.advanceTimersByTimeAsync(0);
    expect(controller.results.value).toHaveLength(0);
    requests[1]?.resolve({ results: [result("new")], nextCursor: null });
    await vi.advanceTimersByTimeAsync(0);
    expect(controller.results.value).toHaveLength(0);
    requests[2]?.resolve({ results: [result("latest")], nextCursor: null });
    await vi.advanceTimersByTimeAsync(0);
    expect(controller.results.value.map((item) => item.msgId)).toEqual([
      "latest",
    ]);
    expect(controller.activeRequestCount.value).toBe(0);
  });

  it("传递复合筛选并去重分页结果", async () => {
    vi.useFakeTimers();
    const first = { results: [result("one")], nextCursor: "cursor-1" };
    const second = {
      results: [result("one"), result("two")],
      nextCursor: null,
    };
    const search = vi
      .fn()
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce(second);
    const controller = new GlobalSearchController(adapterFor(search));

    controller.setFilter({
      query: "needle",
      ownerType: "group",
      ownerId: "group-a",
      topicId: "topic-a",
      speakerAgentId: "speaker-a",
      role: "assistant",
      startTime: 100,
      endTime: 200,
    });
    await vi.advanceTimersByTimeAsync(220);
    await vi.waitFor(() => expect(search).toHaveBeenCalledTimes(1));
    expect(search.mock.calls[0]?.[0]).toMatchObject({
      query: "needle",
      ownerType: "group",
      ownerId: "group-a",
      topicId: "topic-a",
      speakerAgentId: "speaker-a",
      role: "assistant",
      startTime: 100,
      endTime: 200,
    });
    controller.loadMore();
    await vi.waitFor(() => expect(search).toHaveBeenCalledTimes(2));
    expect(search.mock.calls[1]?.[0].cursor).toBe("cursor-1");
    expect(controller.results.value.map((item) => item.msgId)).toEqual([
      "one",
      "two",
    ]);
  });

  it("区分索引不一致并仅在明确操作后重建", async () => {
    const adapter = adapterFor(
      vi.fn(async () => ({ results: [], nextCursor: null })),
    );
    adapter.getIndexStatus = vi.fn(async () =>
      status({ healthy: false, missingCount: 1 }),
    );
    const controller = new GlobalSearchController(adapter);

    await controller.refreshIndexStatus();
    expect(controller.indexState.value).toBe("inconsistent");
    expect(adapter.rebuildIndex).not.toHaveBeenCalled();
    await controller.rebuildIndex();
    expect(adapter.rebuildIndex).toHaveBeenCalledTimes(1);
  });

  it("日期范围倒置时在发起请求前提示并拒绝搜索", async () => {
    vi.useFakeTimers();
    const search = vi.fn(
      async (): Promise<SearchPage> => ({
        results: [],
        nextCursor: null,
      }),
    );
    const controller = new GlobalSearchController(adapterFor(search));

    controller.setFilter({ query: "needle", startTime: 200, endTime: 100 });
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
    expect(search).not.toHaveBeenCalled();
    expect(controller.searchStatus.value).toBe("invalid");
    expect(controller.validationMessage.value).toContain("起始日期");
  });
});
