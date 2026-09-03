import type { Ref } from "vue";
import type { ChatMessage } from "../types/chat";
import type {
  ConversationIdentity,
  ConversationOwnerType,
} from "./chatStoreIdentity";

export type PendingGenerationOptions = {
  requestId?: string;
  registered?: boolean;
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
