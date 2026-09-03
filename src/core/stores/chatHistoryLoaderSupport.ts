import type { ConversationIdentity } from "./chatStoreIdentity";
import { sameConversationIdentity } from "./chatStoreIdentity";
import type { HistoryLoaderDeps } from "./chatHistoryLoader";

export interface HistoryLoaderState {
  currentLoadAbortController: AbortController | null;
  preloadRequestEpoch: number;
  historyEpoch: number;
  activeHistoryEpoch: number;
  activeHistoryIdentity: ConversationIdentity | null;
}

export function isActiveHistoryRequest(
  deps: HistoryLoaderDeps,
  state: HistoryLoaderState,
  identity: ConversationIdentity,
  requestEpoch: number,
  signal: AbortSignal,
): boolean {
  return (
    !signal.aborted &&
    requestEpoch === state.activeHistoryEpoch &&
    deps.isCurrentIdentity(identity)
  );
}

export function shouldIgnoreHistoryPage(
  state: HistoryLoaderState,
  identity: ConversationIdentity,
  offset: number,
  requestEpoch: number,
): boolean {
  return (
    offset > 0 &&
    (requestEpoch !== state.activeHistoryEpoch ||
      !sameConversationIdentity(state.activeHistoryIdentity, identity))
  );
}
