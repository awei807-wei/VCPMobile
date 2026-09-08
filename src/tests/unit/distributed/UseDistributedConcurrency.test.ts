import { defineComponent, h } from "vue";
import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  emitTauriEvent,
  invokeMock,
  listenMock,
  mockInvoke,
} from "@/tests/mocks/tauri";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const status = (session_id: number, state = "connected") => ({
  session_id,
  state,
  connected: state === "connected",
  server_id: `server-${session_id}`,
  client_id: `client-${session_id}`,
  registered_tools: 1,
  last_error: null,
});

async function mountDistributed(resetModules = true) {
  if (resetModules) vi.resetModules();
  const { useDistributed } =
    await import("@/features/distributed/composables/useDistributed");
  let api!: ReturnType<typeof useDistributed>;
  const wrapper = mount(
    defineComponent({
      setup() {
        api = useDistributed();
        return () => h("div");
      },
    }),
  );
  return { api, wrapper };
}

describe("useDistributed 会话状态并发控制", () => {
  beforeEach(() => {
    invokeMock.mockClear();
    listenMock.mockClear();
    mockInvoke("get_distributed_status", () => status(0, "disconnected"));
  });

  it("同一消息号跨 owner 的新事件不会被旧快照覆盖", async () => {
    const snapshot = deferred<unknown>();
    mockInvoke("get_distributed_status", () => snapshot.promise);
    const { api, wrapper } = await mountDistributed();

    const activating = api.activate();
    await Promise.resolve();
    emitTauriEvent("vcp-distributed-status", status(2));
    snapshot.resolve(status(1, "disconnected"));
    await activating;

    expect(api.status.value).toMatchObject({ session_id: 2, connected: true });
    wrapper.unmount();
  });

  it("监听注册失败后回滚所有权，并允许同一实例再次激活", async () => {
    listenMock.mockRejectedValueOnce(new Error("注册失败"));
    const { api, wrapper } = await mountDistributed();

    await api.activate();
    expect(listenMock).toHaveBeenCalledTimes(1);
    await api.activate();

    expect(listenMock).toHaveBeenCalledTimes(2);
    expect(api.status.value.state).toBe("disconnected");
    wrapper.unmount();
  });

  it("最后一个消费者离开时注销监听，挂起注册完成后不留下 ownership", async () => {
    const registration = deferred<() => void>();
    listenMock.mockImplementationOnce(() => registration.promise);
    const { api, wrapper } = await mountDistributed();
    const activating = api.activate();
    await Promise.resolve();

    api.deactivate();
    const stop = vi.fn();
    registration.resolve(stop);
    await activating;

    expect(stop).toHaveBeenCalledTimes(1);
    wrapper.unmount();
  });

  it("多个消费者共享监听，只有最后一个停止才释放", async () => {
    const first = await mountDistributed();
    const second = await mountDistributed(false);
    await Promise.all([first.api.activate(), second.api.activate()]);
    expect(listenMock).toHaveBeenCalledTimes(1);

    const stop = vi.mocked(listenMock).mock.results[0]?.value;
    first.api.deactivate();
    second.api.deactivate();
    first.wrapper.unmount();
    second.wrapper.unmount();
    await Promise.resolve(stop);
  });
});
