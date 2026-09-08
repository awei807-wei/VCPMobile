import { ref } from "vue";
import { beforeEach, describe, expect, it } from "vitest";
import {
  channelInstances,
  invokeMock,
  mockInvoke,
} from "@/tests/mocks/tauri";
import {
  createTopicListActions,
  type TopicStoreContext,
} from "@/core/stores/topicListManagerActions";
import type { Topic } from "@/core/stores/topicTypes";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function makeTopic(id: string, ownerId: string, ownerType: "agent" | "group"): Topic {
  return {
    id,
    ownerId,
    ownerType,
    name: id,
    createdAt: 1,
  };
}

function makeContext(): TopicStoreContext {
  return {
    topics: ref([]),
    loading: ref(false),
    currentAgentId: ref(null),
    currentOwnerKey: ref(null),
    requestEpoch: ref(0),
    sessionStore: {} as TopicStoreContext["sessionStore"],
    notificationStore: {
      addNotification: () => undefined,
    } as unknown as TopicStoreContext["notificationStore"],
  };
}

describe("话题列表并发门禁", () => {
  beforeEach(() => {
    channelInstances.length = 0;
    invokeMock.mockClear();
  });

  it("A→B→A 重载时拒绝旧 A 的迟到分片和终态", async () => {
    const context = makeContext();
    const actions = createTopicListActions(context);
    const completion = deferred<void>();
    mockInvoke("get_topics_streamed", () => completion.promise);

    const first = actions.loadTopicList("owner-a", "agent");
    const firstChannel = channelInstances[0];
    const middle = actions.loadTopicList("owner-b", "agent");
    const latest = actions.loadTopicList("owner-a", "agent");
    const latestChannel = channelInstances[2];

    firstChannel.emit([makeTopic("A1", "owner-a", "agent")]);
    latestChannel.emit([makeTopic("A2", "owner-a", "agent")]);
    completion.resolve();
    await Promise.all([first, middle, latest]);

    expect(context.topics.value.map((topic) => topic.id)).toEqual(["A2"]);
    expect(context.requestEpoch.value).toBe(3);
  });

  it("失效缓存后拒绝旧请求的迟到分片和终态", async () => {
    const context = makeContext();
    const actions = createTopicListActions(context);
    mockInvoke("get_topics_streamed", () => undefined);

    const loading = actions.loadTopicList("owner-a", "agent");
    const channel = channelInstances[0];
    channel.emit([makeTopic("before", "owner-a", "agent")]);
    actions.invalidateAllTopicCaches();
    channel.emit([makeTopic("late", "owner-a", "agent")]);
    await loading;

    expect(context.topics.value).toEqual([]);
    expect(context.loading.value).toBe(false);
    expect(context.requestEpoch.value).toBe(2);
  });

  it("切换 owner 后拒绝迟到的创建结果写入当前列表", async () => {
    const context = makeContext();
    context.currentOwnerKey.value = "agent:owner-a";
    context.requestEpoch.value = 7;
    const actions = createTopicListActions(context);
    const created = deferred<Topic>();
    mockInvoke("create_topic", () => created.promise);
    mockInvoke("get_topics_streamed", () => undefined);

    const creating = actions.createTopic("owner-a", "agent", "迟到话题");
    const reloading = actions.loadTopicList("owner-b", "agent");
    created.resolve(makeTopic("late-create", "owner-a", "agent"));
    await creating;
    await reloading;

    expect(context.currentOwnerKey.value).toBe("agent:owner-b");
    expect(context.topics.value).toEqual([]);
    expect(context.requestEpoch.value).toBe(8);
  });
});
