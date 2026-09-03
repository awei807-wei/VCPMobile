import { beforeEach, describe, expect, it, vi } from "vitest";
import { tauriSearchAdapter } from "../../../features/globalsearch/searchAdapter";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

beforeEach(() => {
  invokeMock.mockReset();
});

describe("全局搜索适配器", () => {
  it("拒绝缺失必要字段的搜索结果而不静默丢弃", async () => {
    invokeMock.mockResolvedValue({
      results: [{ msgId: "message-a", topicId: "topic-a" }],
      nextCursor: null,
    });

    await expect(tauriSearchAdapter.search({ query: "测试" })).rejects.toThrow(
      "搜索结果格式无效",
    );
  });

  it("完整读取索引解码错误与诊断状态", async () => {
    invokeMock.mockResolvedValue({
      available: true,
      schemaValid: true,
      tokenizerValid: true,
      liveCount: 4,
      indexedCount: 4,
      missingCount: 0,
      orphanCount: 0,
      duplicateCount: 0,
      staleCount: 0,
      decodeErrorCount: 1,
      healthy: false,
      diagnostic: "FTS_CONTENT_DECODE_FAILED",
    });

    await expect(tauriSearchAdapter.getIndexStatus()).resolves.toMatchObject({
      decodeErrorCount: 1,
      healthy: false,
      diagnostic: "FTS_CONTENT_DECODE_FAILED",
    });
  });

  it("验证锚点窗口并保留下一页偏移", async () => {
    invokeMock.mockResolvedValue({
      messages: [{ id: "message-a", role: "user", timestamp: 1 }],
      nextOffset: 13,
      hasMoreHistory: true,
    });

    await expect(
      tauriSearchAdapter.loadHistoryAround({
        ownerId: "owner-a",
        ownerType: "agent",
        topicId: "topic-a",
        anchorMessageId: "message-a",
      }),
    ).resolves.toMatchObject({ nextOffset: 13, hasMoreHistory: true });
  });
});
