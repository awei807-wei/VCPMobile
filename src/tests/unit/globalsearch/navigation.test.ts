import { describe, expect, it, vi } from "vitest";
import {
  findMessageElement,
  loadSearchJumpHistory,
} from "../../../features/globalsearch/navigation";
import type { SearchAdapter } from "../../../features/globalsearch/types";

const result = {
  msgId: "message:with[brackets]",
  topicId: "topic:with:colon",
  ownerType: "agent" as const,
  ownerId: "owner-a",
  role: "user",
  speakerAgentId: null,
  timestamp: 100,
  topicTitle: "话题",
  snippet: "摘要",
  rank: null,
  requestId: 1,
};

describe("全局搜索导航", () => {
  it("按完整身份加载锚点并安全定位含特殊字符的消息标识", async () => {
    const adapter: SearchAdapter = {
      search: vi.fn(),
      getIndexStatus: vi.fn(),
      rebuildIndex: vi.fn(),
      loadHistoryAround: vi.fn(async (input) => ({
        messages: [{ id: input.anchorMessageId, role: "user", timestamp: 1 }],
        nextOffset: 17,
        hasMoreHistory: true,
      })),
    };
    const history = { installAnchoredHistory: vi.fn(() => true) };
    const loaded = await loadSearchJumpHistory(
      result,
      { ownerId: "owner-a", ownerType: "agent", topicId: result.topicId },
      history,
      () => true,
      adapter,
    );
    expect(loaded).toBe(true);
    expect(adapter.loadHistoryAround).toHaveBeenCalledWith({
      ownerId: "owner-a",
      ownerType: "agent",
      topicId: result.topicId,
      anchorMessageId: result.msgId,
      beforeCount: 12,
      afterCount: 12,
    });
    expect(history.installAnchoredHistory).toHaveBeenCalledWith(
      { ownerId: "owner-a", ownerType: "agent", topicId: result.topicId },
      expect.objectContaining({ nextOffset: 17, hasMoreHistory: true }),
    );

    const root = document.createElement("div");
    const first = document.createElement("article");
    first.setAttribute("data-message-id", "other");
    const target = document.createElement("article");
    target.setAttribute("data-message-id", result.msgId);
    root.append(first, target);
    expect(findMessageElement(root, result.msgId)).toBe(target);
  });

  it("锚点消失时关闭跳转且不安装历史窗口", async () => {
    const adapter: SearchAdapter = {
      search: vi.fn(),
      getIndexStatus: vi.fn(),
      rebuildIndex: vi.fn(),
      loadHistoryAround: vi.fn(async () => ({
        messages: [{ id: "other", role: "user", timestamp: 1 }],
        nextOffset: 1,
        hasMoreHistory: false,
      })),
    };
    const history = { installAnchoredHistory: vi.fn(() => true) };
    await expect(
      loadSearchJumpHistory(
        result,
        { ownerId: "owner-a", ownerType: "agent", topicId: result.topicId },
        history,
        () => true,
        adapter,
      ),
    ).resolves.toBe(false);
    expect(history.installAnchoredHistory).not.toHaveBeenCalled();
  });

  it("会话已切换时拒绝安装返回的旧锚点窗口", async () => {
    const adapter: SearchAdapter = {
      search: vi.fn(),
      getIndexStatus: vi.fn(),
      rebuildIndex: vi.fn(),
      loadHistoryAround: vi.fn(async () => ({
        messages: [{ id: result.msgId, role: "user", timestamp: 1 }],
        nextOffset: 1,
        hasMoreHistory: false,
      })),
    };
    const history = { installAnchoredHistory: vi.fn(() => false) };
    await expect(
      loadSearchJumpHistory(
        result,
        { ownerId: "owner-a", ownerType: "agent", topicId: result.topicId },
        history,
        () => true,
        adapter,
      ),
    ).resolves.toBe(false);
    expect(history.installAnchoredHistory).toHaveBeenCalledTimes(1);
  });

  it("同会话的新跳转开始后拒绝安装旧请求返回的历史窗口", async () => {
    let resolveWindow!: (window: {
      messages: { id: string; role: string; timestamp: number }[];
      nextOffset: number;
      hasMoreHistory: boolean;
    }) => void;
    const windowPromise = new Promise<{
      messages: { id: string; role: string; timestamp: number }[];
      nextOffset: number;
      hasMoreHistory: boolean;
    }>((resolve) => {
      resolveWindow = resolve;
    });
    const adapter: SearchAdapter = {
      search: vi.fn(),
      getIndexStatus: vi.fn(),
      rebuildIndex: vi.fn(),
      loadHistoryAround: vi.fn(() => windowPromise),
    };
    const history = { installAnchoredHistory: vi.fn(() => true) };
    let currentRequestId = result.requestId;
    const loading = loadSearchJumpHistory(
      result,
      { ownerId: "owner-a", ownerType: "agent", topicId: result.topicId },
      history,
      (requestId) => requestId === currentRequestId,
      adapter,
    );

    currentRequestId += 1;
    resolveWindow({
      messages: [{ id: result.msgId, role: "user", timestamp: 1 }],
      nextOffset: 1,
      hasMoreHistory: false,
    });

    await expect(loading).resolves.toBe(false);
    expect(history.installAnchoredHistory).not.toHaveBeenCalled();
  });
});
