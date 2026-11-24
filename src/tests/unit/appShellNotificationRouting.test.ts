import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("vue-router", () => ({ useRouter: vi.fn() }));

import { createSystemEventHandler } from "../../core/composables/useAppShell";

const makeContext = (processed: Record<string, unknown> | null) => {
  const addNotification = vi.fn();
  const context = {
    processPayload: vi.fn(() => processed),
    notificationStore: { addNotification },
  };
  // 只驱动 handler 用到的两个字段
  const handler = createSystemEventHandler(context as any);
  return { handler, addNotification };
};

describe("createSystemEventHandler · historyOnly 路由", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
  });

  it("普通 WS 通知只进收纳栏", () => {
    const { handler, addNotification } = makeContext({
      title: "FileOperator 执行成功",
      message: "ok",
      type: "info",
      actions: [],
      silent: false,
      historyOnly: false,
    });
    handler({
      payload: {
        type: "vcp_log",
        data: { tool_name: "FileOperator", status: "success" },
      },
    });
    expect(addNotification).toHaveBeenCalledTimes(1);
    expect(addNotification.mock.calls[0][0].historyOnly).toBe(true);
  });

  it("审批请求保留弹窗", () => {
    const { handler, addNotification } = makeContext({
      title: "🛠️ 审核请求: PowerShellExecutor",
      message: "...",
      type: "warning",
      duration: 0,
      actions: [
        { label: "允许", value: true },
        { label: "拒绝", value: false },
      ],
      silent: false,
      historyOnly: false,
    });
    handler({ payload: { type: "tool_approval_request", data: {} } });
    expect(addNotification.mock.calls[0][0].historyOnly).toBe(false);
    expect(addNotification.mock.calls[0][0].actions).toHaveLength(2);
  });

  it("处理器已标 historyOnly 时保持 true，即便带 actions", () => {
    const { handler, addNotification } = makeContext({
      title: "x",
      message: "x",
      type: "info",
      actions: [{ label: "a", value: true }],
      silent: false,
      historyOnly: true,
    });
    handler({
      payload: {
        type: "vcp_log",
        data: { id: "vcp_sync_connection_status", status: "error" },
      },
    });
    expect(addNotification.mock.calls[0][0].historyOnly).toBe(true);
  });

  it("silent / null 结果不入库", () => {
    const silent = makeContext({ silent: true });
    silent.handler({ payload: {} });
    expect(silent.addNotification).not.toHaveBeenCalled();

    const empty = makeContext(null);
    empty.handler({ payload: {} });
    expect(empty.addNotification).not.toHaveBeenCalled();
  });
});
