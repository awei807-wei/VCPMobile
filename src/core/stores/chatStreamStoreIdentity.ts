import {
  currentConversationIdentity,
  makeConversationIdentity,
  makeMessageIdentity,
  messageIdentityKey,
  type ConversationIdentity,
} from "./chatStoreIdentity";

export interface ChatSessionIdentitySource {
  currentSelectedItem?: unknown;
  currentTopicId?: unknown;
}

export interface ChatStreamIdentityApi {
  currentIdentity: () => ConversationIdentity | null;
  explicitIdentity: (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => ConversationIdentity | null;
  legacySession: (
    ownerId: string,
    topicId: string,
  ) => ConversationIdentity | null;
  activeMessageKey: (
    identity: ConversationIdentity,
    messageId: string,
  ) => string | null;
}

export function createChatStreamIdentityApi(
  sessionStore: ChatSessionIdentitySource,
): ChatStreamIdentityApi {
  const currentIdentity = () => currentConversationIdentity(sessionStore);

  const explicitIdentity = (
    ownerId: string,
    ownerType: string,
    topicId: string,
  ) => makeConversationIdentity(ownerId, ownerType, topicId);

  const legacySession = (ownerId: string, topicId: string) => {
    const current = currentIdentity();
    if (
      !current ||
      current.ownerId !== ownerId ||
      current.topicId !== topicId
    ) {
      return null;
    }
    return current;
  };

  const activeMessageKey = (
    identity: ConversationIdentity,
    messageId: string,
  ) => {
    const message = makeMessageIdentity(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
      messageId,
    );
    return message ? messageIdentityKey(message) : null;
  };

  return {
    currentIdentity,
    explicitIdentity,
    legacySession,
    activeMessageKey,
  };
}
