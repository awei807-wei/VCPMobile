import { describe, expect, it, vi } from "vitest";
import { ref } from "vue";
import { createHistoryGeneration } from "../../core/stores/chatHistoryGeneration";

function makeGeneration(status: string) {
  const stagedAttachments = [
    {
      type: "application/octet-stream",
      name: "pending.bin",
      size: 1,
      src: "",
      status,
    },
  ];
  const clearStaged = vi.fn();
  const currentChatHistory = ref<any[]>([]);
  const identity = {
    ownerId: "agent-1",
    ownerType: "agent" as const,
    topicId: "topic-1",
  };
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
      preProcessDocuments: vi.fn(),
    },
    assistantStore: { agents: [] },
    settingsStore: { settings: { userName: "User" } },
    topicStore: { topics: [], incrementTopicMsgCount: vi.fn() },
    switchGuardStore: { switching: false },
    currentIdentity: () => identity,
    isCurrentIdentity: () => true,
  };
  return {
    generation: createHistoryGeneration(deps),
    clearStaged,
    currentChatHistory,
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
});
