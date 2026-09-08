// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import {
  cancelUnreadReceipt,
  cancelUnreadReceiptsForTopic,
  createStreamEventProcessor,
} from "../../../core/stores/chatStreamProcessor";
import type { StreamProcessorDeps } from "../../../core/stores/chatStreamProcessorSupport";
import type { TopicUnreadState } from "../../../core/stores/topicListCounters";
import { invokeMock, mockInvoke } from "../../mocks/tauri";
import { useChatSessionStore } from "../../../core/stores/chatSessionStore";
import { useChatStreamStore } from "../../../core/stores/chatStreamStore";
import { useTopicStore } from "../../../core/stores/topicListManager";

const baseContext = {
  ownerType: "agent" as const,
  ownerId: "agent-1",
  topicId: "topic-1",
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function event(
  type: string,
  generation: number,
  messageId = "message-1",
  extra: Record<string, unknown> = {},
) {
  return {
    type,
    generation,
    messageId,
    context: baseContext,
    ...extra,
  };
}

function createDeps(): StreamProcessorDeps {
  return {
    state: {
      activeStreamMessages: new Map(),
      rAFPendingUpdates: new Map(),
      cleanupTimers: new Set(),
      streamingMessageId: { value: null },
      streamingMessageKey: { value: null },
      streamGenerations: new Map(),
      generationWatermarks: new Map(),
    },
    computeShell: () => undefined,
    addSessionStream: vi.fn(),
    removeSessionStream: vi.fn(),
    incrementTopicMsgCount: vi.fn(),
    incrementTopicUnreadCount: vi.fn(),
    currentIdentity: () => null,
    invoke: vi.fn(async () => []),
    isStreamDebugEnabled: () => false,
    recordStreamTrace: vi.fn(),
    streamDebugLog: vi.fn(),
  };
}

describe("逻辑流消息话题计数", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("process_message_content", () => []);
  });

  it("generation 1 被 generation 2 替换时只计数一次", async () => {
    const deps = createDeps();
    const process = createStreamEventProcessor(deps);

    await process(event("thinking", 1));
    await process(event("thinking", 2));
    await process(event("data", 2, "message-1", { chunk: "最新" }));

    expect(deps.incrementTopicMsgCount).toHaveBeenCalledTimes(1);
    expect(deps.incrementTopicUnreadCount).toHaveBeenCalledTimes(1);
    expect(deps.incrementTopicUnreadCount).toHaveBeenCalledWith(
      expect.objectContaining({
        ownerId: "agent-1",
        ownerType: "agent",
        topicId: "topic-1",
      }),
      "message-1",
    );
  });

  it("未读收据失败后后续事件可重试且并发事件去重", async () => {
    const deps = createDeps();
    let rejectFirst!: (error: unknown) => void;
    const firstAttempt = new Promise<boolean>((_, reject) => {
      rejectFirst = reject;
    });
    const submitUnread = vi
      .fn()
      .mockReturnValueOnce(firstAttempt)
      .mockResolvedValueOnce(true);
    deps.incrementTopicUnreadCount = submitUnread;
    const process = createStreamEventProcessor(deps);
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);

    await process(event("thinking", 1));
    await process(event("data", 1, "message-1", { chunk: "并发重放" }));
    expect(submitUnread).toHaveBeenCalledTimes(1);
    expect(deps.state.unreadMessageKeys?.size).toBe(0);
    expect(deps.state.unreadMessageInFlightKeys?.size).toBe(1);

    rejectFirst(new Error("transient failure"));
    await vi.waitFor(() =>
      expect(deps.state.unreadMessageInFlightKeys?.size).toBe(0),
    );
    expect(deps.state.unreadMessageKeys?.size).toBe(0);

    await process(event("data", 1, "message-1", { chunk: "重试" }));
    await vi.waitFor(() => expect(deps.state.unreadMessageKeys?.size).toBe(1));
    expect(submitUnread).toHaveBeenCalledTimes(2);
    consoleError.mockRestore();
  });

  it("流终结期间补账延迟也会自动重试未读收据", async () => {
    const deps = createDeps();
    const firstAttempt = deferred<boolean>();
    const submitUnread = vi
      .fn()
      .mockReturnValueOnce(firstAttempt.promise)
      .mockResolvedValueOnce(true);
    deps.incrementTopicUnreadCount = submitUnread;
    const process = createStreamEventProcessor(deps);

    await process(event("thinking", 1));
    await process(event("end", 1));
    expect(submitUnread).toHaveBeenCalledTimes(1);
    expect(deps.state.unreadMessageInFlightKeys?.size).toBe(1);

    firstAttempt.resolve(false);
    await vi.waitFor(() => expect(submitUnread).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(deps.state.unreadMessageKeys?.size).toBe(1));
    expect(deps.state.unreadMessageInFlightKeys?.size).toBe(0);
  });

  it("消息或话题不存在时立即停止未读收据重试", async () => {
    const deps = createDeps();
    const submitUnread = vi
      .fn()
      .mockRejectedValue(new Error("消息不存在、已删除或身份不唯一"));
    deps.incrementTopicUnreadCount = submitUnread;
    const process = createStreamEventProcessor(deps);
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);

    await process(event("thinking", 1));
    await vi.waitFor(() => expect(submitUnread).toHaveBeenCalledTimes(1));
    await vi.waitFor(() =>
      expect(deps.state.unreadMessageRetryStates?.size ?? 0).toBe(0),
    );

    expect(deps.state.unreadMessageRetryTimers?.size ?? 0).toBe(0);
    expect(deps.state.unreadMessageRetryStates?.size ?? 0).toBe(0);
    expect(consoleError).toHaveBeenCalled();
    consoleError.mockRestore();
  });

  it("未读收据按退避间隔重试，并可在流结束后成功", async () => {
    vi.useFakeTimers();
    try {
      const deps = createDeps();
      const submitUnread = vi
        .fn()
        .mockReturnValueOnce(false)
        .mockReturnValueOnce(false)
        .mockReturnValueOnce(true);
      deps.incrementTopicUnreadCount = submitUnread;
      const process = createStreamEventProcessor(deps);

      await process(event("end", 1));
      expect(submitUnread).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(49);
      expect(submitUnread).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(submitUnread).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(99);
      expect(submitUnread).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(1);
      expect(submitUnread).toHaveBeenCalledTimes(3);
      expect(deps.state.unreadMessageKeys?.size).toBe(1);
      expect(deps.state.unreadMessageRetryTimers?.size).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("达到最大尝试次数后释放未读收据重试状态", async () => {
    vi.useFakeTimers();
    try {
      const deps = createDeps();
      const submitUnread = vi.fn().mockReturnValue(false);
      deps.incrementTopicUnreadCount = submitUnread;
      const process = createStreamEventProcessor(deps);
      const consoleError = vi
        .spyOn(console, "error")
        .mockImplementation(() => undefined);

      await process(event("end", 1));
      await vi.runAllTimersAsync();

      expect(submitUnread).toHaveBeenCalledTimes(5);
      expect(deps.state.unreadMessageInFlightKeys?.size).toBe(0);
      expect(deps.state.unreadMessageRetryTimers?.size).toBe(0);
      expect(deps.state.unreadMessageRetryStates?.size).toBe(0);
      expect(consoleError).toHaveBeenCalled();
      consoleError.mockRestore();
    } finally {
      vi.useRealTimers();
    }
  });

  it("删除消息或话题会取消对应的未读收据 timer", async () => {
    vi.useFakeTimers();
    try {
      const deps = createDeps();
      const submitUnread = vi.fn().mockReturnValue(false);
      deps.incrementTopicUnreadCount = submitUnread;
      const process = createStreamEventProcessor(deps);

      await process(event("thinking", 1, "message-1"));
      await process(event("thinking", 1, "message-2"));
      expect(deps.state.unreadMessageRetryTimers?.size).toBe(2);

      cancelUnreadReceipt(deps.state, baseContext, "message-1");
      expect(deps.state.unreadMessageRetryTimers?.size).toBe(1);
      cancelUnreadReceiptsForTopic(deps.state, baseContext);
      await vi.advanceTimersByTimeAsync(1_000);

      expect(submitUnread).toHaveBeenCalledTimes(2);
      expect(deps.state.unreadMessageRetryTimers?.size).toBe(0);
      expect(deps.state.unreadMessageRetryStates?.size).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("取消后活动流的迟到帧不会重新创建 receipt retry", async () => {
    vi.useFakeTimers();
    try {
      const deps = createDeps();
      const submitUnread = vi.fn().mockReturnValue(false);
      deps.incrementTopicUnreadCount = submitUnread;
      const process = createStreamEventProcessor(deps);

      await process(event("thinking", 1));
      cancelUnreadReceipt(deps.state, baseContext, "message-1");
      await process(event("data", 1, "message-1", { chunk: "迟到" }));
      await process(event("end", 1, "message-1"));
      await vi.runAllTimersAsync();

      expect(submitUnread).toHaveBeenCalledTimes(1);
      expect(deps.state.unreadMessageRetryTimers?.size ?? 0).toBe(0);
      expect(deps.state.unreadMessageRetryStates?.size ?? 0).toBe(0);
      expect(deps.state.unreadMessageReceiptTombstones?.size ?? 0).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("store dispose 时在途未读 IPC 失败不会触发二次提交", async () => {
    const first = deferred<TopicUnreadState>();
    mockInvoke("increment_topic_unread_count", () => first.promise);
    const sessionStore = useChatSessionStore();
    sessionStore.currentSelectedItem = { id: baseContext.ownerId, type: "agent" };
    sessionStore.currentTopicId = baseContext.topicId;
    const topicStore = useTopicStore();
    topicStore.topics = [{
      id: baseContext.topicId,
      ownerId: baseContext.ownerId,
      ownerType: "agent",
      name: "topic",
      createdAt: 1,
      msgCount: 0,
    }];
    const streamStore = useChatStreamStore();

    await streamStore.processStreamEvent(event("thinking", 1));
    await vi.waitFor(() =>
      expect(
        invokeMock.mock.calls.filter(
          ([command]) => command === "increment_topic_unread_count",
        ),
      ).toHaveLength(1),
    );
    streamStore.$dispose();
    first.reject(new Error("dispose during IPC"));
    await topicStore.flushUnreadMutations();

    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "increment_topic_unread_count",
      ),
    ).toHaveLength(1);
  });

  it("冷恢复重建 skeleton 不会在同一运行时重复提交消息收据", async () => {
    const deps = createDeps();
    const process = createStreamEventProcessor(deps);

    await process(event("thinking", 1));
    await process(event("thinking", 2, "message-1"));
    await process(event("end", 2, "message-1"));

    expect(deps.incrementTopicMsgCount).toHaveBeenCalledTimes(1);
    expect(deps.incrementTopicUnreadCount).toHaveBeenCalledTimes(1);
  });

  it("独立 Pinia 生命周期中的冷恢复会重新提交收据并由后端去重", async () => {
    const prepare = () => {
      const sessionStore = useChatSessionStore();
      sessionStore.currentSelectedItem = { id: "current-agent", type: "agent" };
      sessionStore.currentTopicId = "current-topic";
      const topicStore = useTopicStore();
      topicStore.topics = [{
        id: baseContext.topicId,
        ownerId: baseContext.ownerId,
        ownerType: "agent",
        name: "topic",
        createdAt: 1,
        msgCount: 0,
      }];
      return { topicStore, streamStore: useChatStreamStore() };
    };

    mockInvoke("increment_topic_unread_count", (args = {}) => ({
      ownerId: args.ownerId,
      ownerType: args.ownerType,
      topicId: args.topicId,
      unread: true,
      unreadCount: 1,
    }));
    const first = prepare();
    await first.streamStore.processStreamEvent(event("thinking", 1));
    await first.topicStore.flushUnreadMutations();
    expect(
      invokeMock.mock.calls.filter(([command]) => command === "increment_topic_unread_count"),
    ).toHaveLength(1);
    first.streamStore.$dispose();

    invokeMock.mockClear();
    setActivePinia(createPinia());
    const second = prepare();
    await second.streamStore.processStreamEvent(
      event("thinking", 2, "message-1"),
    );
    await second.topicStore.flushUnreadMutations();
    expect(
      invokeMock.mock.calls.filter(([command]) => command === "increment_topic_unread_count"),
    ).toHaveLength(1);
    second.streamStore.$dispose();
  });

  it("非当前会话的 unread 后端 invoke 在恢复链中仅一次", async () => {
    const sessionStore = useChatSessionStore();
    sessionStore.currentSelectedItem = { id: "current-agent", type: "agent" };
    sessionStore.currentTopicId = "current-topic";
    const topicStore = useTopicStore();
    topicStore.topics = [{
      id: baseContext.topicId,
      ownerId: baseContext.ownerId,
      ownerType: "agent",
      name: "topic",
      createdAt: 1,
      msgCount: 0,
    }];
    mockInvoke("increment_topic_unread_count", (args = {}) => ({
      ownerId: args.ownerId,
      ownerType: args.ownerType,
      topicId: args.topicId,
      unread: true,
      unreadCount: 1,
    }));

    const streamStore = useChatStreamStore();
    await streamStore.processStreamEvent(event("thinking", 1));
    await streamStore.processStreamEvent(
      event("thinking", 2, "message-1"),
    );
    await topicStore.flushUnreadMutations();

    expect(
      invokeMock.mock.calls.filter(([command]) => command === "increment_topic_unread_count"),
    ).toHaveLength(1);
  });

  it("不同复合消息身份独立计数", async () => {
    const deps = createDeps();
    const process = createStreamEventProcessor(deps);
    await process(event("thinking", 1, "message-1"));
    await process(event("thinking", 1, "message-2"));
    await process({
      ...event("thinking", 1, "message-1"),
      context: { ownerType: "group", ownerId: "agent-1", topicId: "topic-1" },
    });

    expect(deps.incrementTopicMsgCount).toHaveBeenCalledTimes(3);
    expect(deps.incrementTopicUnreadCount).toHaveBeenCalledTimes(3);
  });

  it("超过旧账本容量后，generation 替换仍不重复计数", async () => {
    const deps = createDeps();
    const process = createStreamEventProcessor(deps);
    for (let index = 0; index < 4097; index += 1) {
      await process(event("end", 1, `message-${index}`));
    }
    await process(event("thinking", 2, "message-0"));

    expect(deps.incrementTopicMsgCount).toHaveBeenCalledTimes(4097);
    expect(deps.incrementTopicUnreadCount).toHaveBeenCalledTimes(4097);
  });
});
