import { nextTick } from "vue";
import {
  makeConversationIdentity,
  sameConversationIdentity,
  type ConversationIdentity,
} from "../../core/stores/chatStoreIdentity";
import { findMessageElement, loadSearchJumpHistory } from "./navigation";
import type { PendingSearchJump } from "./types";

interface SearchJumpStore {
  takePendingJump(identity: ConversationIdentity): PendingSearchJump | null;
  isJumpRequestCurrent(requestId: number): boolean;
  resolveJump(requestId: number, status: "success" | "invalid" | "error"): void;
}

interface SearchHistoryStore {
  installAnchoredHistory(
    identity: ConversationIdentity,
    window: import("./types").SearchHistoryWindow,
  ): boolean;
}

export async function handlePendingSearchJump(
  identity: ConversationIdentity,
  searchStore: SearchJumpStore,
  historyStore: SearchHistoryStore,
  currentIdentity: () => ConversationIdentity | null,
): Promise<boolean> {
  const pending = searchStore.takePendingJump(identity);
  if (!pending) return false;
  try {
    const loaded = await loadSearchJumpHistory(
      pending,
      identity,
      historyStore,
      (requestId) => searchStore.isJumpRequestCurrent(requestId),
    );
    if (!loaded || !sameConversationIdentity(currentIdentity(), identity)) {
      searchStore.resolveJump(pending.requestId, "invalid");
      return true;
    }
    await nextTick();
    const target = findMessageElement(document, pending.msgId);
    if (!target) {
      searchStore.resolveJump(pending.requestId, "invalid");
      return true;
    }
    target.scrollIntoView?.({ behavior: "smooth", block: "center" });
    searchStore.resolveJump(pending.requestId, "success");
  } catch {
    searchStore.resolveJump(pending.requestId, "error");
  }
  return true;
}

export function currentSearchIdentity(
  selectedItem: { id?: unknown; type?: unknown } | null | undefined,
  topicId: unknown,
): ConversationIdentity | null {
  return makeConversationIdentity(
    selectedItem?.id,
    selectedItem?.type,
    topicId,
  );
}
