import { ref, type Ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";
import {
  createTopicUnreadMutationQueue,
  useTopicListCounters,
  type TopicUnreadState,
} from "@/core/stores/topicListCounters";
import {
  createTopicListActions,
  type TopicStoreContext,
} from "@/core/stores/topicListManagerActions";
import type { Topic } from "@/core/stores/topicTypes";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function makeTopic(
  id: string,
  ownerId: string,
  ownerType: "agent" | "group",
  unreadCount = 0,
  unread = false,
): Topic {
  return {
    id,
    ownerId,
    ownerType,
    name: id,
    createdAt: 1,
    unread,
    unreadCount,
  };
}

function unreadState(
  ownerId: string,
  ownerType: "agent" | "group",
  topicId: string,
  unreadCount: number,
  unread = unreadCount > 0,
): TopicUnreadState {
  return { ownerId, ownerType, topicId, unread, unreadCount };
}

function makeContext(topics: Ref<Topic[]>): TopicStoreContext {
  return {
    topics,
    loading: ref(false),
    currentAgentId: ref(null),
    currentOwnerKey: ref(null),
    requestEpoch: ref(0),
    sessionStore: {} as TopicStoreContext["sessionStore"],
    notificationStore: {
      addNotification: vi.fn(),
    } as unknown as TopicStoreContext["notificationStore"],
  };
}

describe("话题未读权威状态", () => {
  beforeEach(() => {
    invokeMock.mockClear();
  });

  it("同一复合话题的并发递增串行执行且不乐观修改", async () => {
    const first = deferred<TopicUnreadState>();
    const second = deferred<TopicUnreadState>();
    let calls = 0;
    mockInvoke("increment_topic_unread_count", () => {
      calls += 1;
      return calls === 1 ? first.promise : second.promise;
    });
    const topics = ref([makeTopic("topic", "owner", "agent")]);
    const counters = useTopicListCounters(
      topics,
      { currentTopicId: null, currentSelectedItem: null },
      { queue: createTopicUnreadMutationQueue() },
    );

    const firstMutation = counters.incrementTopicUnreadCount({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic",
    });
    const secondMutation = counters.incrementTopicUnreadCount({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic",
    });
    await Promise.resolve();
    expect(calls).toBe(1);
    expect(topics.value[0].unreadCount).toBe(0);

    first.resolve(unreadState("owner", "agent", "topic", 1));
    await vi.waitFor(() => expect(calls).toBe(2));
    expect(calls).toBe(2);
    expect(topics.value[0].unreadCount).toBe(1);

    second.resolve(unreadState("owner", "agent", "topic", 2));
    await Promise.all([firstMutation, secondMutation]);
    expect(topics.value[0].unreadCount).toBe(2);
    expect(topics.value[0].unread).toBe(true);
  });

  it("同一 owner 的不同话题按提交顺序应用聚合，旧结果不会覆盖新结果", async () => {
    const first = deferred<TopicUnreadState>();
    const second = deferred<TopicUnreadState>();
    let calls = 0;
    mockInvoke("increment_topic_unread_count", () => {
      calls += 1;
      return calls === 1 ? first.promise : second.promise;
    });
    const topics = ref([
      makeTopic("topic-a", "owner", "agent"),
      makeTopic("topic-b", "owner", "agent"),
    ]);
    const counters = useTopicListCounters(
      topics,
      { currentTopicId: null, currentSelectedItem: null },
      { queue: createTopicUnreadMutationQueue() },
    );

    const firstMutation = counters.incrementTopicUnreadCount({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic-a",
    });
    const secondMutation = counters.incrementTopicUnreadCount({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic-b",
    });
    await Promise.resolve();
    expect(calls).toBe(1);

    first.resolve(unreadState("owner", "agent", "topic-a", 1));
    await vi.waitFor(() => expect(calls).toBe(2));
    second.resolve(unreadState("owner", "agent", "topic-b", 2));
    await Promise.all([firstMutation, secondMutation]);
    expect(topics.value.map((topic) => topic.unreadCount)).toEqual([1, 2]);
  });

  it("read 与 increment 按入队顺序应用权威结果", async () => {
    const calls: string[] = [];
    mockInvoke("increment_topic_unread_count", () => {
      calls.push("increment");
      return unreadState("owner", "agent", "topic", 1);
    });
    mockInvoke("set_topic_unread", () => {
      calls.push("read");
      return unreadState("owner", "agent", "topic", 0, false);
    });
    const topics = ref([makeTopic("topic", "owner", "agent")]);
    const counters = useTopicListCounters(
      topics,
      { currentTopicId: null, currentSelectedItem: null },
      { queue: createTopicUnreadMutationQueue() },
    );
    const increment = counters.incrementTopicUnreadCount({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic",
    });
    const read = counters.markTopicAsRead({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic",
    });
    await Promise.all([increment, read]);
    expect(calls).toEqual(["increment", "read"]);
    expect(topics.value[0]).toMatchObject({ unread: false, unreadCount: 0 });
  });

  it("increment 在 read 之后到达时保留后端的新计数", async () => {
    const calls: string[] = [];
    mockInvoke("set_topic_unread", () => {
      calls.push("read");
      return unreadState("owner", "agent", "topic", 0, false);
    });
    mockInvoke("increment_topic_unread_count", () => {
      calls.push("increment");
      return unreadState("owner", "agent", "topic", 1, true);
    });
    const topics = ref([makeTopic("topic", "owner", "agent", 3, true)]);
    const counters = useTopicListCounters(
      topics,
      { currentTopicId: null, currentSelectedItem: null },
      { queue: createTopicUnreadMutationQueue() },
    );

    await Promise.all([
      counters.markTopicAsRead({
        ownerId: "owner",
        ownerType: "agent",
        topicId: "topic",
      }),
      counters.incrementTopicUnreadCount({
        ownerId: "owner",
        ownerType: "agent",
        topicId: "topic",
      }),
    ]);
    expect(calls).toEqual(["read", "increment"]);
    expect(topics.value[0]).toMatchObject({ unread: true, unreadCount: 1 });
  });

  it("失败时记录错误、合并通知并刷新当前 owner", async () => {
    const error = new Error("网络不可用");
    mockInvoke("increment_topic_unread_count", () => Promise.reject(error));
    const notificationStore = { addNotification: vi.fn() };
    const reloadOwnerTopics = vi.fn(async () => undefined);
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    const topics = ref([makeTopic("topic", "owner", "agent")]);
    const counters = useTopicListCounters(
      topics,
      { currentTopicId: null, currentSelectedItem: null },
      {
        queue: createTopicUnreadMutationQueue(),
        notificationStore,
        reloadOwnerTopics,
      },
    );

    await counters.incrementTopicUnreadCount({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic",
    });

    expect(consoleError).toHaveBeenCalled();
    expect(notificationStore.addNotification).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "topic-unread-mutation:agent:owner:topic",
        type: "error",
      }),
    );
    expect(reloadOwnerTopics).toHaveBeenCalledWith("owner", "agent");
    expect(topics.value[0].unreadCount).toBe(0);
    consoleError.mockRestore();
  });

  it("未读 IPC 失败时等待 reconciliation 后自动重试", async () => {
    const refresh = deferred<void>();
    let calls = 0;
    mockInvoke("increment_topic_unread_count", () => {
      calls += 1;
      return calls === 1
        ? Promise.reject(new Error("首次 IPC 失败"))
        : unreadState("owner", "agent", "topic", 1);
    });
    const refreshOwnerUnreadCount = vi.fn(() => refresh.promise);
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    const counters = useTopicListCounters(
      ref([makeTopic("topic", "owner", "agent")]),
      { currentTopicId: null, currentSelectedItem: null },
      {
        queue: createTopicUnreadMutationQueue(),
        refreshOwnerUnreadCount,
      },
    );

    const mutation = counters.incrementTopicUnreadCount({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic",
    });
    await vi.waitFor(() => expect(refreshOwnerUnreadCount).toHaveBeenCalled());
    expect(calls).toBe(1);

    refresh.resolve();
    await mutation;
    expect(calls).toBe(2);
    consoleError.mockRestore();
  });

  it("未读 IPC 首次在途失败时取消会阻止 reconciliation 后的二次提交", async () => {
    const first = deferred<TopicUnreadState>();
    let calls = 0;
    mockInvoke("increment_topic_unread_count", () => {
      calls += 1;
      return first.promise;
    });
    const active = { value: true };
    const counters = useTopicListCounters(
      ref([makeTopic("topic", "owner", "agent")]),
      { currentTopicId: null, currentSelectedItem: null },
      { queue: createTopicUnreadMutationQueue() },
    );

    const mutation = counters.incrementTopicUnreadCount(
      {
        ownerId: "owner",
        ownerType: "agent",
        topicId: "topic",
      },
      "message-1",
      () => active.value,
    );
    await vi.waitFor(() => expect(calls).toBe(1));
    active.value = false;
    first.reject(new Error("首次 IPC 失败"));

    await mutation;
    expect(calls).toBe(1);
  });

  it("相同 owner id 和 topic id 的 Agent/Group 状态互不覆盖", async () => {
    mockInvoke("increment_topic_unread_count", (args) => {
      const ownerType = args?.ownerType as "agent" | "group";
      return unreadState("same-owner", ownerType, "shared", 4);
    });
    const topics = ref([
      makeTopic("shared", "same-owner", "agent"),
      makeTopic("shared", "same-owner", "group"),
    ]);
    const counters = useTopicListCounters(
      topics,
      { currentTopicId: null, currentSelectedItem: null },
      { queue: createTopicUnreadMutationQueue() },
    );

    await Promise.all([
      counters.incrementTopicUnreadCount({
        ownerId: "same-owner",
        ownerType: "agent",
        topicId: "shared",
      }),
      counters.incrementTopicUnreadCount({
        ownerId: "same-owner",
        ownerType: "group",
        topicId: "shared",
      }),
    ]);
    expect(topics.value.map((topic) => topic.unreadCount)).toEqual([4, 4]);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("手动设置未读状态只应用后端返回的计数", async () => {
    mockInvoke("set_topic_unread", () =>
      unreadState("owner", "agent", "topic", 9, true),
    );
    const topics = ref([makeTopic("topic", "owner", "agent")]);
    const context = makeContext(topics);
    const actions = createTopicListActions(context);

    await actions.setTopicUnread("owner", "agent", "topic", true);
    expect(topics.value[0]).toMatchObject({ unread: true, unreadCount: 9 });
  });

  it("删除话题后刷新 Agent/Group owner 聚合徽标", async () => {
    mockInvoke("delete_topic", () => undefined);
    const topics = ref([makeTopic("topic", "owner", "group", 3, true)]);
    const context = makeContext(topics);
    const refreshOwnerUnreadCount = vi.fn(async () => undefined);
    context.refreshOwnerUnreadCount = refreshOwnerUnreadCount;
    const actions = createTopicListActions(context);

    await actions.deleteTopic("owner", "group", "topic");

    expect(refreshOwnerUnreadCount).toHaveBeenCalledWith("owner", "group");
    expect(topics.value).toEqual([]);
  });

  it("删除话题后取消该话题的未读收据补账", async () => {
    mockInvoke("delete_topic", () => undefined);
    const topics = ref([makeTopic("topic", "owner", "agent")]);
    const context = makeContext(topics);
    const cancelUnreadReceiptsForTopic = vi.fn();
    context.cancelUnreadReceiptsForTopic = cancelUnreadReceiptsForTopic;
    const actions = createTopicListActions(context);

    await actions.deleteTopic("owner", "agent", "topic");

    expect(cancelUnreadReceiptsForTopic).toHaveBeenCalledWith({
      ownerId: "owner",
      ownerType: "agent",
      topicId: "topic",
    });
  });
});
