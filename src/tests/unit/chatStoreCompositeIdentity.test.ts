// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mockInvoke } from "../mocks/tauri";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useTopicStore } from "../../core/stores/topicListManager";
import {
  makeConversationIdentity,
  messageIdentityKey,
  topicIdentityKey,
} from "../../core/stores/chatStoreIdentity";

describe("chat history and stream composite identity", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("append_single_message", () => undefined);
    mockInvoke("process_message_content", () => []);
  });

  it("keeps same topic/message ids isolated between Agent and Group", async () => {
    const assistantStore = useAssistantStore();
    assistantStore.$patch({
      agents: [{ id: "shared", name: "Agent", model: "model" }],
      groups: [{ id: "shared", name: "Group", members: [] }],
    });
    const sessionStore = useChatSessionStore();
    sessionStore.currentSelectedItem = { id: "shared", type: "agent" };
    sessionStore.currentTopicId = "topic";
    const topicStore = useTopicStore();
    topicStore.topics = [
      {
        id: "topic",
        ownerId: "shared",
        ownerType: "agent",
        name: "Agent topic",
        createdAt: 1,
        msgCount: 0,
      },
      {
        id: "topic",
        ownerId: "shared",
        ownerType: "group",
        name: "Group topic",
        createdAt: 2,
        msgCount: 0,
      },
    ];

    const streamStore = useChatStreamStore();
    await streamStore.processStreamEvent({
      type: "data",
      messageId: "message",
      context: { ownerType: "agent", ownerId: "shared", topicId: "topic" },
      chunk: "agent",
    });
    await streamStore.processStreamEvent({
      type: "data",
      messageId: "message",
      context: { ownerType: "group", groupId: "shared", topicId: "topic" },
      chunk: "group",
    });

    expect(streamStore.activeStreamMessages.size).toBe(2);
    expect(
      streamStore.getActiveStreamMessage("shared", "agent", "topic", "message")
        ?.content,
    ).toBe("agent");
    expect(
      streamStore.getActiveStreamMessage("shared", "group", "topic", "message")
        ?.content,
    ).toBe("group");
    expect(streamStore.sessionActiveStreams).toHaveProperty(
      topicIdentityKey({
        ownerId: "shared",
        ownerType: "agent",
        topicId: "topic",
      }),
    );
    expect(streamStore.sessionActiveStreams).toHaveProperty(
      topicIdentityKey({
        ownerId: "shared",
        ownerType: "group",
        topicId: "topic",
      }),
    );
    expect(topicStore.topics[0]).toMatchObject({
      ownerType: "agent",
      msgCount: 1,
    });
    expect(topicStore.topics[0]).not.toHaveProperty("unreadCount");
    expect(topicStore.topics[1]).toMatchObject({
      ownerType: "group",
      msgCount: 1,
      unreadCount: 1,
      unread: true,
    });
  });

  it("drops ownerId-only events and does not invent an Agent namespace", async () => {
    const sessionStore = useChatSessionStore();
    sessionStore.currentSelectedItem = { id: "owner", type: "agent" };
    sessionStore.currentTopicId = "topic";
    const streamStore = useChatStreamStore();

    await streamStore.processStreamEvent({
      type: "data",
      messageId: "ambiguous",
      context: { ownerId: "owner", topicId: "topic" },
      chunk: "must be discarded",
    });
    expect(streamStore.activeStreamMessages.size).toBe(0);
    expect(
      streamStore.isMessageActiveInSession("owner", "topic", "ambiguous"),
    ).toBe(false);
  });

  it("uses encoded full identities so delimiter-bearing ids cannot collide", () => {
    const agent = makeConversationIdentity("a:b", "agent", "topic")!;
    const group = makeConversationIdentity("a", "group", "b:topic")!;
    expect(topicIdentityKey(agent)).not.toBe(topicIdentityKey(group));
    expect(messageIdentityKey({ ...agent, messageId: "m:x" })).toContain(
      "m%3Ax",
    );
    expect(makeConversationIdentity("owner", undefined, "topic")).toBeNull();
  });
});
