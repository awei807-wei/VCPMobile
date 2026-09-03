import type { Router } from "vue-router";
import {
  makeMessageIdentity,
  type ConversationIdentity,
} from "../../core/stores/chatStoreIdentity";
import { tauriSearchAdapter } from "./searchAdapter";
import type {
  PendingSearchJump,
  SearchAdapter,
  SearchHistoryWindow,
  SearchResult,
  TargetStatus,
} from "./types";

export interface SearchNavigationStore {
  beginJump(result: SearchResult): number | null;
  resolveJump(
    requestId: number,
    status: Exclude<TargetStatus, "idle" | "loading">,
  ): void;
}

export interface SearchNavigationDeps {
  router: Router;
  sessionStore: {
    selectTopicById(
      ownerId: string,
      ownerType: "agent" | "group",
      topicId: string,
    ): Promise<void>;
  };
  layoutStore: {
    setLeftDrawer(open: boolean): void;
    setRightDrawer(open: boolean): void;
  };
  searchStore: SearchNavigationStore;
}

export async function navigateToSearchResult(
  result: SearchResult,
  deps: SearchNavigationDeps,
): Promise<"invalid" | "pending" | "error"> {
  const requestId = deps.searchStore.beginJump(result);
  if (requestId === null) return "invalid";
  const identity = makeMessageIdentity(
    result.ownerId,
    result.ownerType,
    result.topicId,
    result.msgId,
  );
  if (!identity) return "invalid";
  try {
    await openChatRoute(deps.router);
    await deps.sessionStore.selectTopicById(
      identity.ownerId,
      identity.ownerType,
      identity.topicId,
    );
    deps.layoutStore.setLeftDrawer(false);
    deps.layoutStore.setRightDrawer(false);
    return "pending";
  } catch {
    deps.searchStore.resolveJump(requestId, "error");
    return "error";
  }
}

async function openChatRoute(router: Router): Promise<void> {
  if (router.currentRoute.value.path !== "/chat") await router.push("/chat");
}

export async function loadSearchJumpHistory(
  pending: PendingSearchJump,
  identity: ConversationIdentity,
  historyStore: SearchHistoryStore,
  adapter: SearchAdapter = tauriSearchAdapter,
): Promise<boolean> {
  const target = makeMessageIdentity(
    pending.ownerId,
    pending.ownerType,
    pending.topicId,
    pending.msgId,
  );
  if (
    !target ||
    target.ownerId !== identity.ownerId ||
    target.ownerType !== identity.ownerType ||
    target.topicId !== identity.topicId
  ) {
    return false;
  }
  const window = await adapter.loadHistoryAround({
    ownerId: target.ownerId,
    ownerType: target.ownerType,
    topicId: target.topicId,
    anchorMessageId: target.messageId,
    beforeCount: 12,
    afterCount: 12,
  });
  if (!window.messages.some((message) => message.id === target.messageId))
    return false;
  return historyStore.installAnchoredHistory(identity, window);
}

export interface SearchHistoryStore {
  installAnchoredHistory(
    identity: ConversationIdentity,
    window: SearchHistoryWindow,
  ): boolean;
}

/** 只遍历固定 data 属性并逐项比较，避免把用户创建的消息 ID 拼进 CSS 选择器。 */
export function findMessageElement(
  root: ParentNode,
  messageId: string,
): HTMLElement | null {
  const elements = root.querySelectorAll<HTMLElement>("[data-message-id]");
  return (
    Array.from(elements).find(
      (element) => element.getAttribute("data-message-id") === messageId,
    ) ?? null
  );
}
