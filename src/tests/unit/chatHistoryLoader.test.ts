import { ref } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatMessage, HistoryChunk } from "../../core/types/chat";
import {
  createHistoryLoader,
  type HistoryLoaderDeps,
} from "../../core/stores/chatHistoryLoader";
import {
  sameConversationIdentity,
  type ConversationIdentity,
} from "../../core/stores/chatStoreIdentity";
import {
  createHistoryStreamSettlement,
  HISTORY_STREAM_SETTLE_TIMEOUT_MS,
} from "../../core/stores/historyStreamSettlement";
import {
  channelInstances,
  mockInvoke,
  type MockChannel,
} from "../mocks/tauri";

const identity: ConversationIdentity = {
  ownerId: "agent-1",
  ownerType: "agent",
  topicId: "topic-1",
};

function message(id: string, timestamp: number): ChatMessage {
  return { id, role: "user", timestamp };
}

function createFixture() {
  let activeIdentity: ConversationIdentity | null = identity;
  const deps: HistoryLoaderDeps = {
    currentChatHistory: ref<ChatMessage[]>([]),
    loading: ref(false),
    historyOffset: ref(0),
    hasMoreHistory: ref(true),
    isLoadingHistory: ref(false),
    preloadedHistory: ref(null),
    sessionStore: {
      currentTopicId: identity.topicId,
      currentSelectedItem: {
        id: identity.ownerId,
        type: identity.ownerType,
      },
    },
    streamStore: {
      getActiveStreamMessage: vi.fn(() => undefined),
    },
    attachmentStore: {
      resolveMessageAssets: vi.fn(),
    },
    currentIdentity: () => activeIdentity,
    isCurrentIdentity: (candidate) =>
      sameConversationIdentity(candidate, activeIdentity),
  };

  return {
    deps,
    loader: createHistoryLoader(deps),
    setActiveIdentity(next: ConversationIdentity | null) {
      activeIdentity = next;
    },
  };
}

async function primeHistory(fixture: ReturnType<typeof createFixture>) {
  const initialMessages = Array.from({ length: 5 }, (_, index) =>
    message(`initial-${index}`, index),
  );
  mockInvoke("load_chat_history", () => initialMessages);
  await fixture.loader.loadHistoryPaginated(
    identity.ownerId,
    identity.ownerType,
    identity.topicId,
  );
  expect(fixture.deps.historyOffset.value).toBe(initialMessages.length);
  expect(fixture.deps.hasMoreHistory.value).toBe(true);
}

function historyChunk(id: string, isLast: boolean): HistoryChunk {
  return {
    message: message(id, Date.now()),
    index: 0,
    is_last: isLast,
  };
}

describe("createHistoryLoader streamed history settlement", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("abort 会立即 settle 并清理超时与 channel 回调", async () => {
    vi.useFakeTimers();
    const controller = new AbortController();
    const detachChannel = vi.fn();
    const settlement = createHistoryStreamSettlement(
      controller.signal,
      detachChannel,
    );

    controller.abort();

    await expect(settlement.completion).resolves.toBe("aborted");
    expect(detachChannel).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("后端缺少终帧时在有界超时后 settle，并清理 late channel 事件", async () => {
    vi.useFakeTimers();
    const fixture = createFixture();
    await primeHistory(fixture);

    let resolveInvocation: ((total: number) => void) | undefined;
    mockInvoke(
      "load_chat_history_streamed",
      () =>
        new Promise<number>((resolve) => {
          resolveInvocation = resolve;
        }),
    );

    const loadingPromise = fixture.loader.loadMoreHistory();
    await Promise.resolve();
    const channel = channelInstances[
      channelInstances.length - 1
    ] as MockChannel<HistoryChunk>;
    channel.emit(historyChunk("partial", false));

    await vi.advanceTimersByTimeAsync(HISTORY_STREAM_SETTLE_TIMEOUT_MS);
    await loadingPromise;

    expect(fixture.deps.isLoadingHistory.value).toBe(false);
    expect(fixture.deps.loading.value).toBe(false);
    expect(fixture.deps.currentChatHistory.value).toHaveLength(5);
    expect(vi.getTimerCount()).toBe(0);

    channel.emit(historyChunk("late-final", true));
    resolveInvocation?.(1);
    await Promise.resolve();
    expect(
      fixture.deps.currentChatHistory.value.some(
        (candidate) => candidate.id === "late-final",
      ),
    ).toBe(false);
  });

  it("超时后的旧终帧不会污染随后启动的新分页请求", async () => {
    vi.useFakeTimers();
    const fixture = createFixture();
    await primeHistory(fixture);

    mockInvoke(
      "load_chat_history_streamed",
      () => new Promise<number>(() => undefined),
    );

    const firstLoad = fixture.loader.loadMoreHistory();
    await Promise.resolve();
    const firstChannel = channelInstances[
      channelInstances.length - 1
    ] as MockChannel<HistoryChunk>;
    await vi.advanceTimersByTimeAsync(HISTORY_STREAM_SETTLE_TIMEOUT_MS);
    await firstLoad;

    const secondLoad = fixture.loader.loadMoreHistory();
    await Promise.resolve();
    const secondChannel = channelInstances[
      channelInstances.length - 1
    ] as MockChannel<HistoryChunk>;
    expect(secondChannel).not.toBe(firstChannel);

    firstChannel.emit(historyChunk("stale-final", true));
    expect(fixture.deps.currentChatHistory.value).toHaveLength(5);

    secondChannel.emit(historyChunk("fresh-final", true));
    await secondLoad;
    expect(fixture.deps.currentChatHistory.value[0]?.id).toBe("fresh-final");
  });

  it("切换到同 ID 的另一 owner 后拒绝迟到终帧", async () => {
    vi.useFakeTimers();
    const fixture = createFixture();
    await primeHistory(fixture);
    mockInvoke(
      "load_chat_history_streamed",
      () => new Promise<number>(() => undefined),
    );

    const loading = fixture.loader.loadMoreHistory();
    await Promise.resolve();
    const channel = channelInstances[
      channelInstances.length - 1
    ] as MockChannel<HistoryChunk>;
    fixture.setActiveIdentity({ ...identity, ownerType: "group" });
    channel.emit(historyChunk("wrong-owner", true));

    expect(fixture.deps.currentChatHistory.value).toHaveLength(5);
    await vi.advanceTimersByTimeAsync(HISTORY_STREAM_SETTLE_TIMEOUT_MS);
    await loading;
    expect(
      fixture.deps.currentChatHistory.value.some(
        (candidate) => candidate.id === "wrong-owner",
      ),
    ).toBe(false);
  });
});
