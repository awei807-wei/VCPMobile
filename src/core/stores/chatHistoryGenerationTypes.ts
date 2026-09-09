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
  /** 发送动作已绑定完整会话身份，即使界面切走也应继续原会话请求。 */
  continueForCapturedIdentity?: boolean;
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
