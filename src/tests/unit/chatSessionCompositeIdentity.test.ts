// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { invokeMock, mockInvoke } from "../mocks/tauri";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";

describe("chat session composite identity", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("同名 ID 的 Agent 与 Group 使用独立的最近话题键", async () => {
    const assistantStore = useAssistantStore();
    assistantStore.$patch({
      agents: [{ id: "shared", name: "Agent", model: "model" }],
      groups: [{ id: "shared", name: "Group", members: [] }],
    });
    mockInvoke("get_topics", (args) => [
      {
        id: args?.ownerType === "agent" ? "agent-topic" : "group-topic",
      },
    ]);

    const sessionStore = useChatSessionStore();
    await sessionStore.selectItem({
      id: "shared",
      name: "Group",
      type: "group",
    });
    await sessionStore.selectItem({
      id: "shared",
      name: "Agent",
      type: "agent",
    });

    expect(sessionStore.lastActiveTopicMap).toEqual({
      "group:shared": "group-topic",
      "agent:shared": "agent-topic",
    });
    expect(sessionStore.currentSelectedItem?.type).toBe("agent");
    expect(sessionStore.currentTopicId).toBe("agent-topic");
  });

  it("拒绝缺失 ownerType 的会话选择", async () => {
    const sessionStore = useChatSessionStore();

    await expect(
      sessionStore.selectItem({ id: "owner", name: "Incomplete" }),
    ).rejects.toThrow("has no valid type");
  });

  it("读取旧版 ownerId 话题键后迁移并持久化为复合键", async () => {
    const assistantStore = useAssistantStore();
    assistantStore.$patch({
      agents: [{ id: "legacy-owner", name: "Agent", model: "model" }],
    });
    mockInvoke("get_topics", () => [{ id: "newest-topic" }]);
    const loadHistory = vi.fn(async () => undefined);
    const sessionStore = useChatSessionStore();
    sessionStore.$patch({
      lastActiveTopicMap: { "legacy-owner": "remembered-topic" },
    });

    await sessionStore.selectItem(
      { id: "legacy-owner", name: "Agent", type: "agent" },
      loadHistory,
    );

    expect(sessionStore.currentTopicId).toBe("remembered-topic");
    expect(sessionStore.lastActiveTopicMap).toEqual({
      "agent:legacy-owner": "remembered-topic",
    });
    expect(loadHistory).toHaveBeenCalledWith(
      "legacy-owner",
      "agent",
      "remembered-topic",
    );
    expect(
      invokeMock.mock.calls.some(([command]) => command === "get_topics"),
    ).toBe(false);
  });
});
