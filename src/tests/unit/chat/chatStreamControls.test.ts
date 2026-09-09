import { computed } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ConversationIdentity } from "../../../core/stores/chatStoreIdentity";
import { createStreamControls } from "../../../core/stores/chatStreamControls";
import { createChatStreamStoreState } from "../../../core/stores/chatStreamStoreState";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

afterEach(() => {
  invokeMock.mockReset();
});

function controlsFor(identity: ConversationIdentity) {
  const state = createChatStreamStoreState();
  return createStreamControls({
    currentIdentity: () => identity,
    activeStreamingIds: computed(() => new Set<string>()),
    activeStreamMessages: state.activeStreamMessages,
    runtimeState: state.runtimeState,
    streamingMessageId: state.streamingMessageId,
    streamingMessageKey: state.streamingMessageKey,
    removeSessionStream: vi.fn(),
    activeMessageKey: () => null,
  });
}

describe("群聊回合中止身份", () => {
  it("同名话题按群组标识发送不同的中止请求", async () => {
    const first = controlsFor({
      ownerId: "group-a",
      ownerType: "group",
      topicId: "shared-topic",
    });
    const second = controlsFor({
      ownerId: "group-b",
      ownerType: "group",
      topicId: "shared-topic",
    });

    await first.stopGroupTurn("shared-topic");
    await second.stopGroupTurn("shared-topic");

    expect(invokeMock).toHaveBeenNthCalledWith(1, "interruptGroupTurn", {
      groupId: "group-a",
      topicId: "shared-topic",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(2, "interruptGroupTurn", {
      groupId: "group-b",
      topicId: "shared-topic",
    });
  });
});
