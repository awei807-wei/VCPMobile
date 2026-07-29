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

const mocks = vi.hoisted(() => ({
  lifecycleStore: null as LifecycleState | null,
  invoke: vi.fn(),
  listen: vi.fn(),
  lifecycleListener: null as ((event: LifecycleEvent) => void) | null,
  unlisten: vi.fn(),
  windowLabel: "main",
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
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

describe("useAppLifecycle stream recovery", () => {
  beforeEach(() => {
    mocks.lifecycleStore = reactive<LifecycleState>({
      state: "BOOTING",
      isBackground: false,
    });
    mocks.invoke.mockReset();
    mocks.listen.mockReset();
    mocks.lifecycleListener = null;
    mocks.unlisten.mockReset();
    mocks.windowLabel = "main";
    mocks.listen.mockImplementation(
      async (_eventName: string, listener: (event: LifecycleEvent) => void) => {
        mocks.lifecycleListener = listener;
        return mocks.unlisten;
      },
    );
    window.history.replaceState({}, "", "/");
    document.documentElement.classList.remove("vcp-paused-animations");
  });

  afterEach(() => {
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

    expect(mocks.invoke).not.toHaveBeenCalled();

    mocks.lifecycleStore!.state = "READY";
    await nextTick();
    await flushPromises();

    expect(commandsCalled()).toEqual(["get_active_generations"]);
  });

  it("将挂载、网络恢复、回前台等并发触发合并为一个恢复任务", async () => {
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

    expect(commandsCalled()).toEqual(["get_active_generations"]);

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

    expect(commandsCalled()).toEqual([
      "get_active_generations",
      "recover_active_generation",
    ]);
    expect(mocks.invoke).toHaveBeenLastCalledWith("recover_active_generation", {
      msgId: "message-1",
    });
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

    expect(commandsCalled()).toEqual(["get_active_generations"]);

    mocks.lifecycleStore!.state = "READY";
    await nextTick();
    await flushPromises();

    expect(commandsCalled()).toEqual([
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

  it("CORE_NOT_READY 作为可恢复竞态跳过，不继续调用恢复命令", async () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);
    mocks.lifecycleStore!.state = "READY";
    mocks.invoke.mockRejectedValue(
      new Error("CORE_NOT_READY: database is initializing"),
    );

    mountLifecycle();
    await flushPromises();

    expect(commandsCalled()).toEqual(["get_active_generations"]);
    expect(consoleError).not.toHaveBeenCalledWith(
      "[useAppLifecycle] Failed to get active generations:",
      expect.anything(),
    );
    consoleError.mockRestore();
  });
});
