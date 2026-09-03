/**
 * Stable identity helpers shared by the history and stream stores.
 *
 * A topic/message id is only meaningful inside its owner namespace.  Keeping
 * the owner type in the key is intentional: Agent and Group ids are allowed
 * to overlap, and no caller may silently fall back to the Agent namespace.
 */
export type ConversationOwnerType = "agent" | "group";

export interface ConversationIdentity {
  ownerId: string;
  ownerType: ConversationOwnerType;
  topicId: string;
}

export interface MessageIdentity extends ConversationIdentity {
  messageId: string;
}

export function isConversationOwnerType(
  value: unknown,
): value is ConversationOwnerType {
  return value === "agent" || value === "group";
}

export function isNonEmptyIdentityPart(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

/**
 * URI encoding prevents delimiter collisions when user-created ids contain
 * `:` or other key punctuation.  The owner type is kept as a literal prefix
 * so diagnostics remain readable while the complete namespace is preserved.
 */
function encodeIdentityPart(value: string): string {
  return encodeURIComponent(value);
}

export function topicIdentityKey(identity: ConversationIdentity): string {
  return [
    identity.ownerType,
    encodeIdentityPart(identity.ownerId),
    encodeIdentityPart(identity.topicId),
  ].join(":");
}

export function messageIdentityKey(identity: MessageIdentity): string {
  return [
    topicIdentityKey(identity),
    encodeIdentityPart(identity.messageId),
  ].join(":");
}

export function makeConversationIdentity(
  ownerId: unknown,
  ownerType: unknown,
  topicId: unknown,
): ConversationIdentity | null {
  if (
    !isNonEmptyIdentityPart(ownerId) ||
    !isConversationOwnerType(ownerType) ||
    !isNonEmptyIdentityPart(topicId)
  ) {
    return null;
  }

  return { ownerId, ownerType, topicId };
}

export function makeMessageIdentity(
  ownerId: unknown,
  ownerType: unknown,
  topicId: unknown,
  messageId: unknown,
): MessageIdentity | null {
  const conversation = makeConversationIdentity(ownerId, ownerType, topicId);
  if (!conversation || !isNonEmptyIdentityPart(messageId)) return null;
  return { ...conversation, messageId };
}

export function sameConversationIdentity(
  left: ConversationIdentity | null | undefined,
  right: ConversationIdentity | null | undefined,
): boolean {
  return (
    !!left &&
    !!right &&
    left.ownerId === right.ownerId &&
    left.ownerType === right.ownerType &&
    left.topicId === right.topicId
  );
}

/**
 * Read the active session without inventing an owner type.  This adapter is
 * used only by legacy UI callbacks that predate the composite-id API; when
 * the active session is incomplete it returns null (fail closed).
 */
export function currentConversationIdentity(session: {
  currentSelectedItem?: unknown;
  currentTopicId?: unknown;
}): ConversationIdentity | null {
  const selected = session.currentSelectedItem as
    | { id?: unknown; type?: unknown }
    | null
    | undefined;
  return makeConversationIdentity(
    selected?.id,
    selected?.type,
    session.currentTopicId,
  );
}
