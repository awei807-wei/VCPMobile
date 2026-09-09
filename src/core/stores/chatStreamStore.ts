import { defineStore } from "pinia";
import { onScopeDispose } from "vue";
import { useChatSessionStore } from "./chatSessionStore";
import { useAssistantStore } from "./assistant";
import { useAvatarStore } from "./avatar";
import { useTopicStore } from "./topicListManager";
import { clearStreamRendering } from "./chatStreamProcessor";
import { createChatStreamActivity } from "./chatStreamStoreActivity";
import { createChatStreamIdentityApi } from "./chatStreamStoreIdentity";
import { createChatStreamRuntime } from "./chatStreamStoreRuntime";
import { createChatStreamStoreState } from "./chatStreamStoreState";

export const useChatStreamStore = defineStore("chatStream", () => {
  const state = createChatStreamStoreState();
  const sessionStore = useChatSessionStore();
  const assistantStore = useAssistantStore();
  const avatarStore = useAvatarStore();
  const topicStore = useTopicStore();
  const identity = createChatStreamIdentityApi(sessionStore);
  const activity = createChatStreamActivity({ state, identity });
  const runtime = createChatStreamRuntime({
    state,
    identity,
    activity,
    assistantStore,
    avatarStore,
    topicStore,
  });
  topicStore.setUnreadReceiptCancellation?.(
    runtime.cancelUnreadReceiptsForTopic,
  );

  onScopeDispose(() => {
    clearStreamRendering(state.runtimeState);
    state.pendingGenerationRequests.value = {};
  });

  return {
    ...state,
    ...activity,
    ...runtime,
  };
});
