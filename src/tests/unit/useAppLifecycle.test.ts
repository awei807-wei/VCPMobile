// @vitest-environment happy-dom

import {
  defineComponent,
  h,
  nextTick,
  reactive,
  type ComponentPublicInstance,
} from "vue";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

interface LifecycleState {
  state: "BOOTING" | "CONNECTING" | "READY";
  isBackground: boolean;
}

interface LifecycleEvent {
  payload: { state: string };
}

const mocks = vi.hoisted(() => {
  class MockChannel<T> {
    onmessage: (event: T) => void = () => undefined;

    emit(event: T) {
      this.onmessage(event);
    }
  }

  return {
    lifecycleStore: null as LifecycleState | null,
    invoke: vi.fn(),
    listen: vi.fn(),
    addPluginListener: vi.fn(),
    lifecycleListener: null as ((event: LifecycleEvent) => void) | null,
    nativeLifecycleListener: null as
      | ((payload: { state?: string }) => void)
      | null,
    unlisten: vi.fn(),
    nativeUnregister: vi.fn(),
    streamProcessEvent: vi.fn(),
    streamChannels: [] as MockChannel<unknown>[],
    Channel: MockChannel,
    windowLabel: "main",
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
  addPluginListener: mocks.addPluginListener,
  Channel: mocks.Channel,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: mocks.windowLabel }),
}));

vi.mock("../../core/stores/appLifecycle", () => ({
  useAppLifecycleStore: () => mocks.lifecycleStore,
}));

vi.mock("../../core/stores/chatStreamStore", () => ({
  useChatStreamStore: () => ({
    processStreamEvent: mocks.streamProcessEvent,
  }),
}));

import { useAppLifecycle } from "../../core/composables/useAppLifecycle";

const mountedWrappers: VueWrapper<ComponentPublicInstance>[] = [];

function mountLifecycle() {
  const wrapper = mount(
    defineComponent({
      setup() {
        useAppLifecycle();
        return () => h("div");
      },
    }),
  );
  mountedWrappers.push(wrapper);
  return wrapper;
}

function commandsCalled() {
  return mocks.invoke.mock.calls.map(([command]) => command);
}

function recoveryCommandsCalled() {
  return commandsCalled().filter(
    (command) => command !== "set_app_foreground_state",
  );
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });
  return { promise, resolve, reject };
}

describe("useAppLifecycle stream recovery", () => {
  beforeEach(() => {
    mocks.lifecycleStore = reactive<LifecycleState>({
      state: "BOOTING",
      isBackground: false,
    });
    mocks.invoke.mockReset();
    mocks.listen.mockReset();
    mocks.addPluginListener.mockReset();
    mocks.lifecycleListener = null;
    mocks.nativeLifecycleListener = null;
    mocks.unlisten.mockReset();
    mocks.nativeUnregister.mockReset();
    mocks.streamProcessEvent.mockReset();
    mocks.streamChannels.length = 0;
    mocks.windowLabel = "main";
    mocks.listen.mockImplementation(
      async (_eventName: string, listener: (event: LifecycleEvent) => void) => {
        mocks.lifecycleListener = listener;
        return mocks.unlisten;
      },
    );
    mocks.addPluginListener.mockImplementation(
      async (
        _pluginName: string,
        _eventName: string,
        listener: (payload: { state?: string }) => void,
      ) => {
        mocks.nativeLifecycleListener = listener;
        return { unregister: mocks.nativeUnregister };
      },
    );
    mocks.streamProcessEvent.mockResolvedValue(undefined);
    window.history.replaceState({}, "", "/");
    document.documentElement.classList.remove("vcp-paused-animations");
  });

  afterEach(() => {
    vi.useRealTimers();
    for (const wrapper of mountedWrappers.splice(0)) {
      wrapper.unmount();
    }
  });

  it("前台冷挂载不触发 TDZ，并将恢复延迟到 READY", async () => {
    mocks.invoke.mockResolvedValue([]);

    expect(() => mountLifecycle()).not.toThrow();
    await flushPromises();
    window.dispatchEvent(new Event("online"));
    mocks.lifecycleListener?.({ payload: { state: "resume" } });
    await nextTick();

    expect(recoveryCommandsCalled()).toEqual([]);

    mocks.lifecycleStore!.state = "READY";
    await nextTick();
    await flushPromises();

    expect(recoveryCommandsCalled()).toEqual(["get_active_generations"]);
  });

  it("并发边界先合并，当前任务结束后开启新的恢复 episode", async () => {
    vi.useFakeTimers();
    let resolveActiveGenerations: ((value: unknown[]) => void) | undefined;
    const activeGenerations = new Promise<unknown[]>((resolve) => {
      resolveActiveGenerations = resolve;
    });
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") return activeGenerations;
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();
    window.dispatchEvent(new Event("online"));
    mocks.lifecycleListener?.({ payload: { state: "pause" } });
    mocks.lifecycleListener?.({ payload: { state: "resume" } });
    await nextTick();

    expect(recoveryCommandsCalled()).toEqual(["get_active_generations"]);

    resolveActiveGenerations?.([
      {
        msgId: "message-1",
        topicId: "topic-1",
        ownerId: "agent-1",
        ownerType: "agent",
        createdAt: 1,
      },
    ]);
    await flushPromises();

    expect(recoveryCommandsCalled()).toEqual([
      "get_active_generations",
      "recover_active_generation",
      "get_active_generations",
      "recover_active_generation",
    ]);
    expect(mocks.invoke).toHaveBeenLastCalledWith("recover_active_generation", {
      msgId: "message-1",
      ownerId: "agent-1",
      ownerType: "agent",
      topicId: "topic-1",
    });
    vi.useRealTimers();
  });

  it("同名 Agent/Group 消息按完整复合身份分别恢复", async () => {
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") {
        return Promise.resolve([
          {
            msgId: "shared-message",
            topicId: "shared-topic",
            ownerId: "agent-owner",
            ownerType: "agent",
            createdAt: 1,
          },
          {
            msgId: "shared-message",
            topicId: "shared-topic",
            ownerId: "group-owner",
            ownerType: "group",
            createdAt: 2,
          },
        ]);
      }
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();

    expect(mocks.invoke).toHaveBeenCalledWith("recover_active_generation", {
      msgId: "shared-message",
      ownerId: "agent-owner",
      ownerType: "agent",
      topicId: "shared-topic",
    });
    expect(mocks.invoke).toHaveBeenCalledWith("recover_active_generation", {
      msgId: "shared-message",
      ownerId: "group-owner",
      ownerType: "group",
      topicId: "shared-topic",
    });
  });

  it("恢复 streaming 生成时创建 Channel 并接续同一完整身份", async () => {
    mocks.lifecycleStore!.state = "READY";
    const generation = {
      msgId: "streaming-message",
      topicId: "topic-streaming",
      ownerId: "group-owner",
      ownerType: "group" as const,
      createdAt: 1,
    };
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") {
        return Promise.resolve([generation]);
      }
      if (command === "recover_active_generation") {
        return Promise.resolve({ status: "streaming", generation: 7 });
      }
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();

    expect(mocks.invoke).toHaveBeenCalledWith(
      "resume_stream",
      expect.objectContaining({
        msgId: generation.msgId,
        topicId: generation.topicId,
        ownerId: generation.ownerId,
        ownerType: generation.ownerType,
        expectedGeneration: 7,
        streamChannel: expect.any(mocks.Channel),
      }),
    );

    const streamChannel = mocks.invoke.mock.calls.find(
      ([command]) => command === "resume_stream",
    )?.[1].streamChannel as {
      emit: (event: unknown) => void;
    };
    const event = {
      type: "data",
      messageId: generation.msgId,
      context: {
        topicId: generation.topicId,
        ownerId: generation.ownerId,
        ownerType: generation.ownerType,
      },
      chunk: "恢复内容",
    };
    streamChannel.emit(event);
    await flushPromises();
    expect(mocks.streamProcessEvent).toHaveBeenCalledWith(event, {
      countMessage: false,
    });
  });

  it("恢复流收到终态后清理 Channel，迟到事件不再进入 store", async () => {
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") {
        return Promise.resolve([
          {
            msgId: "terminal-stream",
            topicId: "terminal-topic",
            ownerId: "terminal-owner",
            ownerType: "agent",
            createdAt: 1,
          },
        ]);
      }
      if (command === "recover_active_generation") {
        return Promise.resolve({ status: "streaming", generation: 71 });
      }
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();
    const payload = mocks.invoke.mock.calls.find(
      ([command]) => command === "resume_stream",
    )?.[1] as { streamChannel: { emit: (event: unknown) => void } };
    payload.streamChannel.emit({
      type: "end",
      messageId: "terminal-stream",
      generation: 71,
    });
    await flushPromises();
    payload.streamChannel.emit({
      type: "data",
      messageId: "terminal-stream",
      generation: 71,
      chunk: "迟到内容",
    });
    await flushPromises();

    expect(mocks.streamProcessEvent).toHaveBeenCalledTimes(1);
    expect(mocks.streamProcessEvent).toHaveBeenCalledWith(
      expect.objectContaining({ type: "end" }),
      { countMessage: false },
    );
  });

  it("接续失败只记录当前 owner，不终结同名的其他 owner", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockImplementation((command: string, args?: any) => {
      if (command === "get_active_generations") {
        return Promise.resolve([
          {
            msgId: "shared-message",
            topicId: "shared-topic",
            ownerId: "agent-owner",
            ownerType: "agent",
            createdAt: 1,
          },
          {
            msgId: "shared-message",
            topicId: "shared-topic",
            ownerId: "group-owner",
            ownerType: "group",
            createdAt: 2,
          },
        ]);
      }
      if (command === "recover_active_generation") {
        return Promise.resolve({
          status: "streaming",
          generation: args?.ownerType === "agent" ? 8 : 9,
        });
      }
      if (command === "resume_stream" && args?.ownerType === "agent") {
        return Promise.reject(new Error("接续失败"));
      }
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();

    expect(mocks.invoke).toHaveBeenCalledWith(
      "resume_stream",
      expect.objectContaining({ ownerId: "agent-owner", ownerType: "agent" }),
    );
    expect(mocks.invoke).toHaveBeenCalledWith(
      "resume_stream",
      expect.objectContaining({ ownerId: "group-owner", ownerType: "group" }),
    );
    expect(consoleError).toHaveBeenCalledWith(
      expect.stringContaining("agent-owner"),
      expect.anything(),
    );
    consoleError.mockRestore();
  });

  it("两条永不结束的恢复流按完整 key 同时启动", async () => {
    mocks.lifecycleStore!.state = "READY";
    const generations = [
      {
        msgId: "long-agent",
        topicId: "topic-long",
        ownerId: "owner-agent",
        ownerType: "agent" as const,
        createdAt: 1,
      },
      {
        msgId: "long-group",
        topicId: "topic-long",
        ownerId: "owner-group",
        ownerType: "group" as const,
        createdAt: 2,
      },
    ];
    const resumes = new Map<string, ReturnType<typeof deferred<void>>>();
    const resumePayloads: any[] = [];
    mocks.invoke.mockImplementation((command: string, args?: any) => {
      if (command === "get_active_generations")
        return Promise.resolve(generations);
      if (command === "recover_active_generation") {
        return Promise.resolve({
          status: "streaming",
          generation: args.ownerType === "agent" ? 31 : 32,
        });
      }
      if (command === "resume_stream") {
        resumePayloads.push(args);
        const wait = deferred<void>();
        resumes.set(args.msgId, wait);
        return wait.promise;
      }
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();

    expect(resumePayloads).toHaveLength(2);
    expect(resumePayloads.map((payload) => payload.expectedGeneration)).toEqual(
      [31, 32],
    );
    expect(resumePayloads.map((payload) => payload.ownerType)).toEqual([
      "agent",
      "group",
    ]);
    resumes.get("long-agent")?.resolve(undefined);
    resumes.get("long-group")?.resolve(undefined);
    await flushPromises();
  });

  it("恢复运行中再次触发会记录边界并在 finally 后立即重新枚举", async () => {
    vi.useFakeTimers();
    mocks.lifecycleStore!.state = "READY";
    const resumeWait = deferred<void>();
    let queryCount = 0;
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") {
        queryCount += 1;
        return queryCount === 1
          ? Promise.resolve([
              {
                msgId: "pending-message",
                topicId: "pending-topic",
                ownerId: "pending-owner",
                ownerType: "agent",
                createdAt: 1,
              },
            ])
          : Promise.resolve([]);
      }
      if (command === "recover_active_generation") {
        return Promise.resolve({ status: "streaming", generation: 41 });
      }
      if (command === "resume_stream") return resumeWait.promise;
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();
    expect(queryCount).toBe(1);
    window.dispatchEvent(new Event("online"));
    await flushPromises();
    expect(queryCount).toBe(1);

    resumeWait.resolve(undefined);
    await flushPromises();
    expect(queryCount).toBe(2);
  });

  it("未决枚举期间的 resume 边界不会被空结果吞掉", async () => {
    mocks.lifecycleStore!.state = "READY";
    const firstQuery = deferred<unknown[]>();
    let queryCount = 0;
    const recoveryLogs = vi
      .spyOn(console, "log")
      .mockImplementation(() => undefined);
    mocks.invoke.mockImplementation((command: string) => {
      if (command !== "get_active_generations") return Promise.resolve();
      queryCount += 1;
      return queryCount === 1 ? firstQuery.promise : Promise.resolve([]);
    });

    mountLifecycle();
    await flushPromises();
    window.dispatchEvent(new Event("online"));
    mocks.lifecycleListener?.({ payload: { state: "resume" } });
    await flushPromises();
    expect(queryCount).toBe(1);

    firstQuery.resolve([]);
    await flushPromises();
    expect(queryCount).toBe(2);
    expect(recoveryLogs).toHaveBeenCalledWith(
      expect.stringContaining("处理挂起的恢复边界：lifecycle-resume"),
    );
    recoveryLogs.mockRestore();
  });

  it("卸载后旧 Channel 的事件被 epoch 门禁丢弃", async () => {
    mocks.lifecycleStore!.state = "READY";
    const resumeWait = deferred<void>();
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") {
        return Promise.resolve([
          {
            msgId: "disposed-message",
            topicId: "disposed-topic",
            ownerId: "disposed-owner",
            ownerType: "group",
            createdAt: 1,
          },
        ]);
      }
      if (command === "recover_active_generation") {
        return Promise.resolve({ status: "streaming", generation: 51 });
      }
      if (command === "resume_stream") return resumeWait.promise;
      return Promise.resolve();
    });

    const wrapper = mountLifecycle();
    await flushPromises();
    const payload = mocks.invoke.mock.calls.find(
      ([command]) => command === "resume_stream",
    )?.[1] as { streamChannel: { emit: (event: unknown) => void } };
    expect(payload?.streamChannel).toBeTruthy();

    wrapper.unmount();
    payload.streamChannel.emit({
      type: "data",
      messageId: "disposed-message",
      context: {
        topicId: "disposed-topic",
        ownerId: "disposed-owner",
        ownerType: "group",
      },
      chunk: "不应进入 store",
    });
    await flushPromises();
    expect(mocks.streamProcessEvent).not.toHaveBeenCalled();
    resumeWait.resolve(undefined);
    await flushPromises();
  });

  it("completed、failed 与 snapshot 使用完整身份终结，缺内容不建空骨架", async () => {
    mocks.lifecycleStore!.state = "READY";
    const generations = [
      {
        msgId: "completed-message",
        topicId: "terminal-topic",
        ownerId: "terminal-agent",
        ownerType: "agent" as const,
        createdAt: 1,
      },
      {
        msgId: "failed-message",
        topicId: "terminal-topic",
        ownerId: "terminal-group",
        ownerType: "group" as const,
        createdAt: 2,
      },
      {
        msgId: "empty-message",
        topicId: "terminal-topic",
        ownerId: "terminal-agent",
        ownerType: "agent" as const,
        createdAt: 3,
      },
    ];
    const consoleWarn = vi
      .spyOn(console, "warn")
      .mockImplementation(() => undefined);
    mocks.invoke.mockImplementation((command: string, args?: any) => {
      if (command === "get_active_generations")
        return Promise.resolve(generations);
      if (command !== "recover_active_generation") return Promise.resolve();
      if (
        args.ownerId === "terminal-agent" &&
        args.msgId === "completed-message"
      ) {
        return Promise.resolve({
          status: "completed",
          generation: 61,
          content: "已完成内容",
          snapshot: [{ type: "paragraph", content: "已完成快照" }],
        });
      }
      if (args.ownerType === "group") {
        return Promise.resolve({
          status: "failed",
          generation: 62,
          content: "失败前内容",
          error: "服务端失败",
          snapshot: [{ type: "paragraph", content: "失败快照" }],
        });
      }
      return Promise.resolve({ status: "completed", generation: 63 });
    });

    mountLifecycle();
    await flushPromises();

    const events = mocks.streamProcessEvent.mock.calls.map(([event]) => event);
    expect(events).toHaveLength(6);
    expect(events[0]).toMatchObject({
      type: "data",
      messageId: "completed-message",
      generation: 61,
      context: {
        ownerType: "agent",
        ownerId: "terminal-agent",
        topicId: "terminal-topic",
      },
    });
    expect(events[1]).toMatchObject({
      type: "aurora",
      messageId: "completed-message",
      generation: 61,
    });
    expect(events[2]).toMatchObject({
      type: "end",
      messageId: "completed-message",
      generation: 61,
    });
    expect(events[3]).toMatchObject({
      type: "data",
      messageId: "failed-message",
      generation: 62,
      context: {
        ownerType: "group",
        ownerId: "terminal-group",
      },
    });
    expect(events[5]).toMatchObject({
      type: "error",
      messageId: "failed-message",
      generation: 62,
      error: "服务端失败",
    });
    expect(consoleWarn).toHaveBeenCalledWith(
      expect.stringContaining("empty-message"),
    );
    consoleWarn.mockRestore();
  });

  it("streaming 缺少正整数 generation 时明确拒绝且不启动 resume", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") {
        return Promise.resolve([
          {
            msgId: "missing-generation",
            topicId: "missing-topic",
            ownerId: "missing-owner",
            ownerType: "agent",
            createdAt: 1,
          },
        ]);
      }
      if (command === "recover_active_generation") {
        return Promise.resolve({ status: "streaming", generation: 0 });
      }
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();

    expect(mocks.invoke).not.toHaveBeenCalledWith(
      "resume_stream",
      expect.anything(),
    );
    expect(consoleError).toHaveBeenCalledWith(
      expect.stringContaining("缺少正整数 generation"),
    );
    consoleError.mockRestore();
  });

  it("终态缺少正整数 generation 时拒绝写入且不误报恢复成功", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") {
        return Promise.resolve([
          {
            msgId: "terminal-missing-generation",
            topicId: "terminal-topic",
            ownerId: "terminal-owner",
            ownerType: "agent",
            createdAt: 1,
          },
        ]);
      }
      if (command === "recover_active_generation") {
        return Promise.resolve({
          status: "completed",
          content: "终态内容但没有 generation",
        });
      }
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();

    expect(mocks.streamProcessEvent).not.toHaveBeenCalled();
    expect(consoleError).toHaveBeenCalledWith(
      expect.stringContaining("终态恢复缺少正整数 generation"),
    );
    consoleError.mockRestore();
  });

  it("原生生命周期事件同步后端状态并复用前后台恢复入口", async () => {
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockResolvedValue([]);

    mountLifecycle();
    await flushPromises();
    mocks.nativeLifecycleListener?.({ state: "pause" });
    await nextTick();
    mocks.nativeLifecycleListener?.({ state: "resume" });
    await flushPromises();

    expect(mocks.invoke).toHaveBeenCalledWith("set_app_foreground_state", {
      isForeground: false,
      requestEpoch: expect.any(Number),
    });
    expect(mocks.invoke).toHaveBeenCalledWith("set_app_foreground_state", {
      isForeground: true,
      requestEpoch: expect.any(Number),
    });
    const foregroundCalls = mocks.invoke.mock.calls
      .filter(([command]) => command === "set_app_foreground_state")
      .map(([, args]) => args.requestEpoch);
    expect(foregroundCalls.length).toBe(3);
    expect(
      foregroundCalls.every(
        (epoch, index) => index === 0 || epoch > foregroundCalls[index - 1],
      ),
    ).toBe(true);
  });

  it("前后台同步共享串行 tail，较新的状态等待旧请求完成", async () => {
    mocks.lifecycleStore!.state = "READY";
    let releaseFirst: (() => void) | undefined;
    const firstSync = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    let foregroundCallCount = 0;
    mocks.invoke.mockImplementation((command: string) => {
      if (command !== "set_app_foreground_state") return Promise.resolve([]);
      foregroundCallCount += 1;
      return foregroundCallCount === 1 ? firstSync : Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();
    mocks.nativeLifecycleListener?.({ state: "pause" });
    mocks.nativeLifecycleListener?.({ state: "resume" });
    await flushPromises();

    expect(foregroundCallCount).toBe(1);
    releaseFirst?.();
    await flushPromises();
    expect(foregroundCallCount).toBe(3);
    expect(
      mocks.invoke.mock.calls
        .filter(([command]) => command === "set_app_foreground_state")
        .map(([, args]) => args.isForeground),
    ).toEqual([true, false, true]);
  });

  it("核心离开 READY 后不调用恢复命令，重新 READY 时再执行", async () => {
    let resolveFirstQuery: ((value: unknown[]) => void) | undefined;
    const firstQuery = new Promise<unknown[]>((resolve) => {
      resolveFirstQuery = resolve;
    });
    let queryCount = 0;
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockImplementation((command: string) => {
      if (command === "get_active_generations") {
        queryCount += 1;
        if (queryCount === 1) return firstQuery;
        return Promise.resolve([
          {
            msgId: "message-2",
            topicId: "topic-2",
            ownerId: "agent-2",
            ownerType: "agent",
            createdAt: 2,
          },
        ]);
      }
      return Promise.resolve();
    });

    mountLifecycle();
    await flushPromises();
    mocks.lifecycleStore!.state = "CONNECTING";
    await nextTick();
    resolveFirstQuery?.([
      {
        msgId: "message-2",
        topicId: "topic-2",
        ownerId: "agent-2",
        ownerType: "agent",
        createdAt: 2,
      },
    ]);
    await flushPromises();

    expect(recoveryCommandsCalled()).toEqual(["get_active_generations"]);

    mocks.lifecycleStore!.state = "READY";
    await nextTick();
    await flushPromises();

    expect(recoveryCommandsCalled()).toEqual([
      "get_active_generations",
      "get_active_generations",
      "recover_active_generation",
    ]);
  });

  it.each([
    { name: "assistant 路由", href: "/#/assistant", label: "main" },
    { name: "assistant 窗口标签", href: "/", label: "assistant" },
    { name: "floating 查询参数", href: "/?mode=floating", label: "main" },
  ])("$name 不注册主窗口恢复逻辑", async ({ href, label }) => {
    window.history.replaceState({}, "", href);
    mocks.windowLabel = label;
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockResolvedValue([]);

    mountLifecycle();
    await flushPromises();
    window.dispatchEvent(new Event("online"));
    await nextTick();

    expect(mocks.listen).not.toHaveBeenCalled();
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("CORE_NOT_READY 使用可取消的有界退避直到核心恢复", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.lifecycleStore!.state = "READY";
    let recoveryAttempts = 0;
    mocks.invoke.mockImplementation((command: string) => {
      if (command !== "get_active_generations") return Promise.resolve();
      recoveryAttempts += 1;
      if (recoveryAttempts < 3) {
        return Promise.reject(
          new Error("CORE_NOT_READY: database is initializing"),
        );
      }
      return Promise.resolve([]);
    });

    vi.useFakeTimers();
    try {
      mountLifecycle();
      await flushPromises();

      expect(recoveryCommandsCalled()).toEqual(["get_active_generations"]);
      await vi.advanceTimersByTimeAsync(250);
      await flushPromises();
      expect(recoveryAttempts).toBe(2);
      await vi.advanceTimersByTimeAsync(500);
      await flushPromises();
      expect(recoveryAttempts).toBe(3);
      expect(consoleError).not.toHaveBeenCalledWith(
        "[useAppLifecycle] Failed to get active generations:",
        expect.anything(),
      );
    } finally {
      vi.useRealTimers();
      consoleError.mockRestore();
    }
  });

  it("卸载恢复控制器会取消待执行的 CORE_NOT_READY 计时器", async () => {
    mocks.lifecycleStore!.state = "READY";
    let recoveryAttempts = 0;
    mocks.invoke.mockImplementation((command: string) => {
      if (command !== "get_active_generations") return Promise.resolve();
      recoveryAttempts += 1;
      return Promise.reject(
        new Error("CORE_NOT_READY: database is initializing"),
      );
    });

    vi.useFakeTimers();
    try {
      const wrapper = mountLifecycle();
      await flushPromises();
      expect(recoveryAttempts).toBe(1);
      wrapper.unmount();
      await vi.advanceTimersByTimeAsync(10_000);
      await flushPromises();
      expect(recoveryAttempts).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("CORE_NOT_READY 单个恢复 episode 最多尝试十次并允许新前台事件重开", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.lifecycleStore!.state = "READY";
    let recoveryAttempts = 0;
    mocks.invoke.mockImplementation((command: string) => {
      if (command !== "get_active_generations") return Promise.resolve();
      recoveryAttempts += 1;
      return Promise.reject(
        new Error("CORE_NOT_READY: database is initializing"),
      );
    });

    vi.useFakeTimers();
    try {
      mountLifecycle();
      await flushPromises();
      await vi.advanceTimersByTimeAsync(60_001);
      await flushPromises();

      expect(recoveryAttempts).toBe(10);
      expect(consoleError).toHaveBeenCalledWith(
        expect.stringContaining("达到上限（10 次）"),
      );

      window.dispatchEvent(new Event("online"));
      await flushPromises();
      expect(recoveryAttempts).toBe(11);
    } finally {
      vi.useRealTimers();
      consoleError.mockRestore();
    }
  });

  it("第十次恢复仍在途时 online 边界会在结束后重开 episode", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.lifecycleStore!.state = "READY";
    let recoveryAttempts = 0;
    const tenthAttempt = deferred<unknown[]>();
    mocks.invoke.mockImplementation((command: string) => {
      if (command !== "get_active_generations") return Promise.resolve();
      recoveryAttempts += 1;
      if (recoveryAttempts < 10) {
        return Promise.reject(
          new Error("CORE_NOT_READY: database is initializing"),
        );
      }
      if (recoveryAttempts === 10) return tenthAttempt.promise;
      return Promise.resolve([]);
    });

    vi.useFakeTimers();
    try {
      mountLifecycle();
      await flushPromises();
      await vi.advanceTimersByTimeAsync(60_001);
      await flushPromises();
      expect(recoveryAttempts).toBe(10);

      window.dispatchEvent(new Event("online"));
      await nextTick();
      await flushPromises();
      expect(recoveryAttempts).toBe(10);

      tenthAttempt.resolve([]);
      await flushPromises();
      expect(recoveryAttempts).toBe(11);
    } finally {
      vi.useRealTimers();
      consoleError.mockRestore();
    }
  });
});
