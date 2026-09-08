import type { ParsedStreamEvent, StreamState } from "./chatStreamProcessorSupport";
import {
  getMessageTombstones,
  getTopicTombstones,
} from "./chatStreamUnreadReceiptState";
import {
  messageIdentityKey,
  topicIdentityKey,
  type ConversationIdentity,
} from "./chatStoreIdentity";

function sameConversation(
  left: ConversationIdentity,
  right: ConversationIdentity,
): boolean {
  return (
    left.ownerId === right.ownerId &&
    left.ownerType === right.ownerType &&
    left.topicId === right.topicId
  );
}

function hasActiveReceiptForTopic(state: StreamState, topicKey: string): boolean {
  const records = state.unreadMessageReceiptRecords;
  if (!records) return false;
  for (const messageKey of state.streamGenerations.keys()) {
    const record = records.get(messageKey);
    if (record && topicIdentityKey(record.identity) === topicKey) return true;
  }
  return false;
}

function cancelUnreadReceiptKey(state: StreamState, messageKey: string): void {
  const timer = state.unreadMessageRetryTimers?.get(messageKey);
  if (timer !== undefined) clearTimeout(timer);
  state.unreadMessageRetryTimers?.delete(messageKey);
  state.unreadMessageRetryStates?.delete(messageKey);
  state.unreadMessageKeys?.delete(messageKey);
  state.unreadMessageFailedKeys?.delete(messageKey);
  if (state.streamGenerations.has(messageKey)) {
    getMessageTombstones(state).add(messageKey);
  } else {
    getMessageTombstones(state).delete(messageKey);
    state.unreadMessageReceiptRecords?.delete(messageKey);
  }
  // A promise cannot be cancelled, so retain an existing in-flight marker
  // until its finally handler runs. This preserves the one-flight invariant.
}

export function releaseUnreadReceiptTombstonesAfterTerminal(
  state: StreamState,
  parsed: ParsedStreamEvent,
): void {
  const messageTombstones = state.unreadMessageReceiptTombstones;
  const topicTombstones = state.unreadTopicReceiptTombstones;
  const topicKey = topicIdentityKey(parsed.identity);
  const messageWasTombstoned = messageTombstones?.delete(parsed.messageKey);
  if (messageWasTombstoned || topicTombstones?.has(topicKey)) {
    state.unreadMessageReceiptRecords?.delete(parsed.messageKey);
  }
  if (
    topicTombstones?.has(topicKey) &&
    !hasActiveReceiptForTopic(state, topicKey)
  ) {
    topicTombstones.delete(topicKey);
  }
}

export function cancelUnreadReceipt(
  state: StreamState,
  identity: ConversationIdentity,
  messageId: string,
): void {
  cancelUnreadReceiptKey(
    state,
    messageIdentityKey({ ...identity, messageId }),
  );
}

export function cancelUnreadReceiptsForTopic(
  state: StreamState,
  identity: ConversationIdentity,
): void {
  const records = state.unreadMessageReceiptRecords;
  if (!records) return;
  const topicKey = topicIdentityKey(identity);
  let hasActiveStream = false;
  for (const [messageKey, record] of records) {
    if (sameConversation(record.identity, identity)) {
      cancelUnreadReceiptKey(state, messageKey);
      hasActiveStream ||= state.streamGenerations.has(messageKey);
    }
  }
  if (hasActiveStream) getTopicTombstones(state).add(topicKey);
}
