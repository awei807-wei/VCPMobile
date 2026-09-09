import { describe, expect, it, vi } from "vitest";
import { ref } from "vue";
import { createHistoryGeneration } from "../../core/stores/chatHistoryGeneration";
import type { ConversationIdentity } from "../../core/stores/chatStoreIdentity";
import { invokeMock, mockInvoke } from "../mocks/tauri";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function makeGeneration(
  status: string,
  preProcessDocuments?: (attachments: unknown[]) => Promise<void>,
) {
  const preprocess = preProcessDocuments ?? vi.fn(async () => undefined);
  const stagedAttachments = [
    {
      type: "application/pdf",
      name: "pending.pdf",
      size: 1,
      src: "",
      status,
    },
  ];
  const clearStaged = vi.fn();
  const currentChatHistory = ref<any[]>([]);
  const identity: ConversationIdentity = {
    ownerId: "agent-1",
    ownerType: "agent",
    topicId: "topic-1",
  };
  let activeIdentity = identity;
  const deps: any = {
    currentChatHistory,
    editingOriginalMessageId: ref(null),
    sessionStore: {
      currentSelectedItem: { id: "agent-1", type: "agent" },
      currentTopicId: "topic-1",
    },
    streamStore: {
      activeStreamingIds: new Set(),
      addPendingGeneration: vi.fn(),
      removePendingGeneration: vi.fn(),
      computeShell: vi.fn(() => ({})),
    },
    attachmentStore: {
      stagedAttachments,
      clearStaged,
      preProcessDocuments: preprocess,
    },
    assistantStore: { agents: [] },
    settingsStore: { settings: { userName: "User" } },
    topicStore: { topics: [], incrementTopicMsgCount: vi.fn() },
    switchGuardStore: { switching: false },
    currentIdentity: () => activeIdentity,
    isCurrentIdentity: (candidate: ConversationIdentity) =>
      candidate.ownerId === activeIdentity.ownerId &&
      candidate.ownerType === activeIdentity.ownerType &&
      candidate.topicId === activeIdentity.topicId,
  };
  return {
    generation: createHistoryGeneration(deps),
    clearStaged,
    currentChatHistory,
    identity,
    preProcessDocuments: preprocess,
    stagedAttachments,
    streamStore: deps.streamStore,
    topicStore: deps.topicStore,
    switchTo(nextIdentity: ConversationIdentity) {
      activeIdentity = nextIdentity;
      deps.sessionStore.currentSelectedItem = {
        id: nextIdentity.ownerId,
        type: nextIdentity.ownerType,
      };
      deps.sessionStore.currentTopicId = nextIdentity.topicId;
    },
  };
}

describe("历史生成附件门禁", () => {
  it.each(["loading", "processing"])(
    "附件处于 %s 时不清空暂存区也不追加消息",
    async (status) => {
      const fixture = makeGeneration(status);
      await fixture.generation.sendMessage("send");
      expect(fixture.clearStaged).not.toHaveBeenCalled();
      expect(fixture.currentChatHistory.value).toHaveLength(0);
    },
  );

  it("预处理期间切换会话时仍按发送开始时的完整身份持久化", async () => {
    const preprocessing = deferred();
    const fixture = makeGeneration(
      "done",
      vi.fn(() => preprocessing.promise),
    );
    mockInvoke("append_single_message", () => []);
    mockInvoke("handle_agent_chat_message", () => undefined);

    const sending = fixture.generation.sendMessage("保留这条输入");
    expect(fixture.clearStaged).toHaveBeenCalledOnce();
    expect(fixture.preProcessDocuments).toHaveBeenCalledWith(
      fixture.stagedAttachments,
    );

    fixture.switchTo({
      ownerId: "agent-2",
      ownerType: "agent",
      topicId: "topic-2",
    });
    preprocessing.resolve();
    await sending;

    expect(invokeMock).toHaveBeenCalledWith(
      "append_single_message",
      expect.objectContaining({
        ownerId: fixture.identity.ownerId,
        ownerType: fixture.identity.ownerType,
        topicId: fixture.identity.topicId,
        message: expect.objectContaining({
          content: "保留这条输入",
          attachments: fixture.stagedAttachments,
        }),
      }),
    );
    expect(invokeMock).toHaveBeenCalledWith(
      "handle_agent_chat_message",
      expect.objectContaining({
        payload: expect.objectContaining({
          ownerId: fixture.identity.ownerId,
          ownerType: fixture.identity.ownerType,
          topicId: fixture.identity.topicId,
        }),
      }),
    );
    expect(fixture.currentChatHistory.value).toHaveLength(0);
    expect(fixture.topicStore.incrementTopicMsgCount).toHaveBeenCalledWith(
      fixture.identity,
    );
    expect(fixture.streamStore.removePendingGeneration).toHaveBeenCalledWith(
      fixture.identity.ownerId,
      fixture.identity.ownerType,
      fixture.identity.topicId,
      expect.any(String),
    );
  });
});
