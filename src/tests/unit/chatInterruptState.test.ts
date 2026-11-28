// @vitest-environment happy-dom
import { expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { ref } from "vue";
import { mount } from "@vue/test-utils";
import InputActionButtons from "../../features/chat/components/InputActionButtons.vue";
import { mockInvoke, invokeMock } from "../mocks/tauri";
import { createGenerationChannel } from "../../core/stores/chatHistoryGenerationSupport";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";

it("hands off pending to the reply and stays idle after a late interrupted frame", async () => {
  setActivePinia(createPinia());
  const context = {
    ownerId: "owner",
    ownerType: "agent",
    topicId: "topic",
  } as const;
  mockInvoke("increment_topic_unread_count", () => ({
    ...context,
    unread: false,
    unreadCount: 0,
    ownerUnreadCount: 0,
  }));
  const sessionStore = useChatSessionStore();
  sessionStore.currentSelectedItem = { id: "owner", type: "agent" };
  sessionStore.currentTopicId = "topic";
  const streamStore = useChatStreamStore();
  const channel = createGenerationChannel(
    {
      sessionStore,
      streamStore,
      currentChatHistory: ref([]),
      editingOriginalMessageId: ref(null),
      attachmentStore: {},
      assistantStore: {},
      settingsStore: {},
      switchGuardStore: {},
      topicStore: { topics: [] },
      currentIdentity: () => context,
      isCurrentIdentity: () => true,
    },
    context,
    "user",
  );
  streamStore.addPendingGeneration("owner", "agent", "topic", "user");
  expect([...streamStore.activeStreamingIds]).toEqual(["user"]);

  const frame = {
    context,
    type: "data",
    generation: 1,
    messageId: "reply",
    chunk: "first",
  };
  await channel.onmessage(frame);
  expect([...streamStore.activeStreamingIds]).toEqual(["reply"]);
  mockInvoke("interruptRequest", () => ({ success: true }));
  await streamStore.stopMessage("reply");
  expect(invokeMock).toHaveBeenCalledWith("interruptRequest", {
    ...context,
    messageId: "reply",
  });
  expect(streamStore.activeStreamingIds.size).toBe(0);
  await channel.onmessage({ ...frame, chunk: "late" });
  expect(streamStore.activeStreamingIds.size).toBe(0);
  await channel.onmessage({
    ...frame,
    type: "end",
    finishReason: "cancelled_by_user",
    blocks: [],
  });
  expect(streamStore.getCurrentActiveStreamMessage("reply")?.finishReason).toBe(
    "cancelled_by_user",
  );
  await channel.onmessage({ ...frame, generation: 2 });
  expect([...streamStore.activeStreamingIds]).toEqual(["reply"]);
});
it("shows send after stop even with an empty draft, and enables it when typed", async () => {
  const wrapper = mount(InputActionButtons, {
    props: {
      visible: false,
      hasContent: false,
      isGenerating: true,
      sendDisabled: false,
    },
    global: { plugins: [createPinia()], directives: { longpress: {} } },
  });
  expect(wrapper.get('button[aria-label="停止生成"]').element).toHaveProperty(
    "disabled",
    false,
  );
  await wrapper.setProps({ isGenerating: false });
  const send = wrapper.get('button[aria-label="发送消息"]');
  expect(send.element).toHaveProperty("disabled", true);
  await wrapper.setProps({ hasContent: true });
  expect(send.element).toHaveProperty("disabled", false);
  await send.trigger("click");
  expect(wrapper.emitted("action")).toHaveLength(1);
  wrapper.unmount();
});
