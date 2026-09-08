import { ref } from "vue";
import { describe, expect, it, vi } from "vitest";
import { createHistoryGeneration } from "../../core/stores/chatHistoryGeneration";
import { createHistoryMutations } from "../../core/stores/chatHistoryMutations";
import { createHistoryRegeneration } from "../../core/stores/chatHistoryRegeneration";
import type { ChatMessage } from "../../core/types/chat";
import type { ConversationIdentity } from "../../core/stores/chatStoreIdentity";
import { invokeMock, mockInvoke } from "../mocks/tauri";

const identity: ConversationIdentity = {
  ownerId: "shared-owner",
  ownerType: "group",
  topicId: "shared-topic",
};

function message(id: string, timestamp: number, role: ChatMessage["role"] = "user"):
  ChatMessage {
  return { id, role, timestamp, content: id };
}

function createMutationFixture() {
  const currentChatHistory = ref<ChatMessage[]>([
    message("first", 1),
    message("anchor", 2),
    message("last", 3),
  ]);
  const topicStore = { setTopicMsgCount: vi.fn() };
  const cancelUnreadReceipt = vi.fn();
  const deps = {
    currentChatHistory,
    sessionStore: {},
    topicStore,
    currentIdentity: () => identity,
    isCurrentIdentity: (candidate: ConversationIdentity) => candidate === identity,
    cancelUnreadReceipt,
  };
  return {
    currentChatHistory,
    topicStore,
    cancelUnreadReceipt,
    mutations: createHistoryMutations(deps),
  };
}

function createGenerationFixture() {
  const currentChatHistory = ref<ChatMessage[]>([
    message("user", 1),
    message("anchor", 2),
    message("old-response", 3, "assistant"),
  ]);
  const editingOriginalMessageId = ref<string | null>("anchor");
  let activeIdentity: ConversationIdentity | null = identity;
  const sessionStore = {
    currentSelectedItem: { id: identity.ownerId, type: identity.ownerType },
    currentTopicId: identity.topicId,
  };
  const topicStore = {
    topics: [],
    setTopicMsgCount: vi.fn(),
    incrementTopicMsgCount: vi.fn(),
  };
  const deps = {
    currentChatHistory,
    editingOriginalMessageId,
    sessionStore,
    streamStore: {
      addPendingGeneration: vi.fn(),
      removePendingGeneration: vi.fn(),
      cancelUnreadReceipt: vi.fn(),
      computeShell: vi.fn(() => undefined),
      processStreamEvent: vi.fn(),
    },
    attachmentStore: {
      stagedAttachments: [],
      clearStaged: vi.fn(),
      preProcessDocuments: vi.fn(),
    },
    assistantStore: { agents: [] },
    settingsStore: { settings: { userName: "User", vcpServerUrl: "", vcpApiKey: "" } },
    topicStore,
    switchGuardStore: { switching: false },
    currentIdentity: () => identity,
    isCurrentIdentity: (candidate: ConversationIdentity) => candidate === activeIdentity,
  };
  return {
    currentChatHistory,
    editingOriginalMessageId,
    topicStore,
    streamStore: deps.streamStore,
    sessionStore,
    switchTo(nextIdentity: ConversationIdentity) {
      activeIdentity = nextIdentity;
      sessionStore.currentSelectedItem = {
        id: nextIdentity.ownerId,
        type: nextIdentity.ownerType,
      };
      sessionStore.currentTopicId = nextIdentity.topicId;
    },
    generation: createHistoryGeneration(deps),
  };
}

function createRegenerationFixture() {
  const currentChatHistory = ref<ChatMessage[]>([
    message("target", 1, "user"),
    message("response", 2, "assistant"),
  ]);
  const streamStore = {
    addPendingGeneration: vi.fn(),
    removePendingGeneration: vi.fn(),
    cancelUnreadReceipt: vi.fn(),
    processStreamEvent: vi.fn(),
  };
  const deps = {
    currentChatHistory,
    sessionStore: {},
    streamStore,
    topicStore: { setTopicMsgCount: vi.fn() },
    currentIdentity: () => identity,
    isCurrentIdentity: () => true,
    summarizeTopic: vi.fn(async () => undefined),
  };
  return {
    streamStore,
    regeneration: createHistoryRegeneration(deps),
  };
}

describe("历史 mutation 的复合身份与权威结果", () => {
  it("截断使用 anchorMessageId/includeAnchor，并按后端删除身份与 msgCount 收敛", async () => {
    const fixture = createMutationFixture();
    mockInvoke("truncate_history_after_timestamp", () => ({
      deletedIds: ["anchor", "last"],
      msgCount: 1,
    }));

    await fixture.mutations.deleteMessage("anchor", true);

    expect(invokeMock).toHaveBeenCalledWith(
      "truncate_history_after_timestamp",
      {
        ownerId: identity.ownerId,
        ownerType: identity.ownerType,
        topicId: identity.topicId,
        anchorMessageId: "anchor",
        includeAnchor: true,
      },
    );
    expect(fixture.currentChatHistory.value.map(({ id }) => id)).toEqual([
      "first",
    ]);
    expect(fixture.topicStore.setTopicMsgCount).toHaveBeenCalledWith(identity, 1);
    expect(fixture.cancelUnreadReceipt).toHaveBeenCalledWith(identity, "anchor");
    expect(fixture.cancelUnreadReceipt).toHaveBeenCalledWith(identity, "last");
  });

  it("单消息删除携带完整 owner 身份，并接受后端权威 msgCount", async () => {
    const fixture = createMutationFixture();
    mockInvoke("delete_messages", () => ({
      deletedIds: ["anchor"],
      msgCount: 2,
    }));

    await fixture.mutations.deleteMessage("anchor");

    expect(invokeMock).toHaveBeenCalledWith("delete_messages", {
      ownerId: identity.ownerId,
      ownerType: identity.ownerType,
      topicId: identity.topicId,
      msgIds: ["anchor"],
    });
    expect(fixture.currentChatHistory.value.map(({ id }) => id)).toEqual([
      "first",
      "last",
    ]);
    expect(fixture.topicStore.setTopicMsgCount).toHaveBeenCalledWith(identity, 2);
    expect(fixture.cancelUnreadReceipt).toHaveBeenCalledWith(identity, "anchor");
  });
});

describe("编辑重生成的锚点 mutation", () => {
  it("使用单一原子 mutation 保留编辑消息，并传递复合身份到生成命令", async () => {
    const fixture = createGenerationFixture();
    mockInvoke("edit_message_and_truncate_history", () => ({
      deletedIds: ["old-response"],
      msgCount: 2,
      blocks: [],
    }));
    mockInvoke("handle_group_chat_message", () => undefined);

    await fixture.generation.sendMessage("edited content");

    expect(invokeMock).toHaveBeenCalledWith(
      "edit_message_and_truncate_history",
      {
        ownerId: identity.ownerId,
        ownerType: identity.ownerType,
        topicId: identity.topicId,
        anchorMessageId: "anchor",
        message: expect.objectContaining({
          id: "anchor",
          content: "edited content",
        }),
      },
    );
    expect(fixture.topicStore.setTopicMsgCount).toHaveBeenCalledWith(identity, 2);
    expect(fixture.streamStore.cancelUnreadReceipt).toHaveBeenCalledWith(
      identity,
      "old-response",
    );
    expect(fixture.currentChatHistory.value).toEqual([
      expect.objectContaining({ id: "user" }),
      expect.objectContaining({
        id: "anchor",
        content: "edited content",
      }),
    ]);
    expect(invokeMock).not.toHaveBeenCalledWith(
      "truncate_history_after_timestamp",
      expect.anything(),
    );
    expect(invokeMock).not.toHaveBeenCalledWith("append_single_message", expect.anything());
    expect(invokeMock).toHaveBeenCalledWith(
      "handle_group_chat_message",
      expect.objectContaining({
        payload: expect.objectContaining({
          groupId: identity.ownerId,
          ownerId: identity.ownerId,
          ownerType: identity.ownerType,
          topicId: identity.topicId,
        }),
      }),
    );
    expect(fixture.editingOriginalMessageId.value).toBeNull();
    expect(fixture.streamStore.addPendingGeneration).toHaveBeenCalledWith(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      "anchor",
    );
    expect(fixture.streamStore.removePendingGeneration).toHaveBeenCalledWith(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      "anchor",
    );
  });

  it("截断拒绝时恢复 editing 身份并清理复合 pending", async () => {
    const fixture = createGenerationFixture();
    mockInvoke("edit_message_and_truncate_history", () => {
      throw new Error("mutation rejected");
    });

    await expect(fixture.generation.sendMessage("edited content")).rejects.toThrow("mutation rejected");

    expect(fixture.editingOriginalMessageId.value).toBe("anchor");
    expect(fixture.streamStore.removePendingGeneration).toHaveBeenCalledWith(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      "anchor",
    );
    expect(fixture.currentChatHistory.value.map(({ id }) => id)).toEqual([
      "user",
      "anchor",
      "old-response",
    ]);
  });

  it("非法截断 DTO 被拒绝且不改变编辑历史", async () => {
    const fixture = createGenerationFixture();
    mockInvoke("edit_message_and_truncate_history", () => ({
      msgCount: 2,
      deletedIds: ["old-response", 3],
    }));

    await expect(fixture.generation.sendMessage("edited content")).rejects.toThrow(
      "后端返回的消息 mutation 结果无效",
    );

    expect(fixture.editingOriginalMessageId.value).toBe("anchor");
    expect(fixture.currentChatHistory.value.map(({ id }) => id)).toEqual([
      "user",
      "anchor",
      "old-response",
    ]);
  });

  it("等待截断期间切换会话时不污染新会话本地状态", async () => {
    const fixture = createGenerationFixture();
    const nextIdentity: ConversationIdentity = {
      ownerId: "other-owner",
      ownerType: "agent",
      topicId: "other-topic",
    };
    mockInvoke("edit_message_and_truncate_history", () => {
      fixture.switchTo(nextIdentity);
      return { msgCount: 1, deletedIds: ["old-response"] };
    });

    await fixture.generation.sendMessage("edited content");

    expect(fixture.currentChatHistory.value.map(({ id }) => id)).toEqual([
      "user",
      "anchor",
      "old-response",
    ]);
    expect(fixture.editingOriginalMessageId.value).toBe("anchor");
    expect(fixture.topicStore.setTopicMsgCount).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "append_single_message",
      expect.anything(),
    );
    expect(fixture.streamStore.removePendingGeneration).toHaveBeenCalledWith(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      "anchor",
    );
  });

  it("编辑锚点持久化失败时保留旧正文、editing 身份并拒绝启动生成", async () => {
    const fixture = createGenerationFixture();
    mockInvoke("edit_message_and_truncate_history", () => {
      throw new Error("mutation rejected");
    });

    await expect(fixture.generation.sendMessage("edited content")).rejects.toThrow("mutation rejected");

    expect(fixture.currentChatHistory.value).toEqual([
      expect.objectContaining({ id: "user" }),
      expect.objectContaining({ id: "anchor", content: "anchor" }),
      expect.objectContaining({ id: "old-response" }),
    ]);
    expect(fixture.editingOriginalMessageId.value).toBe("anchor");
    expect(fixture.topicStore.setTopicMsgCount).not.toHaveBeenCalled();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "handle_group_chat_message",
      expect.anything(),
    );
    expect(fixture.streamStore.removePendingGeneration).toHaveBeenCalledWith(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      "anchor",
    );
  });

  it("编辑身份存在但本地锚点消失时 fail-closed，不回退新消息发送", async () => {
    const fixture = createGenerationFixture();
    fixture.currentChatHistory.value = fixture.currentChatHistory.value.filter(
      (item) => item.id !== "anchor",
    );

    await expect(fixture.generation.sendMessage("edited content")).rejects.toThrow(
      "编辑目标消息未在当前历史记录中找到",
    );
    expect(fixture.editingOriginalMessageId.value).toBe("anchor");
    expect(invokeMock).not.toHaveBeenCalled();
    expect(fixture.streamStore.addPendingGeneration).not.toHaveBeenCalled();
  });

  it("截断成功后生成失败仍保留已提交编辑并清理 pending", async () => {
    const fixture = createGenerationFixture();
    mockInvoke("edit_message_and_truncate_history", () => ({
      msgCount: 2,
      deletedIds: ["old-response"],
      blocks: [],
    }));
    mockInvoke("handle_group_chat_message", () => {
      throw new Error("generation failed");
    });

    await fixture.generation.sendMessage("edited content");

    expect(fixture.editingOriginalMessageId.value).toBeNull();
    expect(fixture.currentChatHistory.value.map(({ id }) => id)).toEqual([
      "user",
      "anchor",
    ]);
    expect(fixture.streamStore.removePendingGeneration).toHaveBeenCalledWith(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      "anchor",
    );
  });

  it("重新生成返回 deletedIds 后取消对应的未读收据", async () => {
    const fixture = createRegenerationFixture();
    mockInvoke("regenerate_topic_response", () => ({
      deletedIds: ["response"],
      msgCount: 1,
    }));

    await fixture.regeneration("target");

    expect(fixture.streamStore.cancelUnreadReceipt).toHaveBeenCalledWith(
      identity,
      "response",
    );
  });
});
