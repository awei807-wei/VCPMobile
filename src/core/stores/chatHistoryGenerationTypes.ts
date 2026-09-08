import type { Ref } from "vue";
import type { ChatMessage } from "../types/chat";
import type {
  ConversationIdentity,
  ConversationOwnerType,
} from "./chatStoreIdentity";

export type PendingGenerationOptions = {
  requestId?: string;
  registered?: boolean;
  /** 调用方已持久化用户消息，生成阶段不得再次追加。 */
  userMessagePersisted?: boolean;
  ownerId?: string;
  ownerType?: ConversationOwnerType;
  topicId?: string;
};

export interface HistoryGenerationDeps {
  currentChatHistory: Ref<ChatMessage[]>;
  editingOriginalMessageId: Ref<string | null>;
  sessionStore: any;
  streamStore: any;
  attachmentStore: any;
  assistantStore: any;
  settingsStore: any;
  topicStore: any;
  switchGuardStore: any;
  currentIdentity: () => ConversationIdentity | null;
  isCurrentIdentity: (identity: ConversationIdentity) => boolean;
}
