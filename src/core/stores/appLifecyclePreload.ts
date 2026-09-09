import { useChatHistoryStore } from "./chatHistoryStore";
import type { AppState } from "./appLifecycle";
import type { AppLifecycleStores } from "./appLifecycleStores";

interface PreloadContext {
  stores: AppLifecycleStores;
  getState: () => AppState;
  cleanupConnectionWaiters: () => void;
  setState: (nextState: AppState, reason: string) => void;
  updatePhaseLabel: (label: string) => void;
  markReady: () => void;
  fail: (message: string) => void;
}

export interface PreloadActions {
  startPreloading: () => Promise<void>;
}

async function preloadDependencies(
  context: PreloadContext,
): Promise<unknown[]> {
  const tasks: Promise<unknown>[] = [
    context.stores.settingsStore.fetchSettings(),
    context.stores.assistantStore.fetchAgentsAndGroups(),
    context.stores.avatarStore.preloadAll(),
  ];
  const owner = context.stores.sessionStore.currentSelectedItem;
  if (!owner?.id) return Promise.all(tasks);
  if (owner.type !== "agent" && owner.type !== "group") {
    context.stores.sessionStore.currentSelectedItem = null;
    context.stores.sessionStore.currentTopicId = null;
    console.warn(
      "[Lifecycle] Dropped restored session with incomplete owner identity",
    );
    return Promise.all(tasks);
  }
  console.log(
    `[Lifecycle] Restored session detected for ${owner.type} ${owner.id}, preloading topic list...`,
  );
  tasks.push(context.stores.topicStore.loadTopicList(owner.id, owner.type));
  return Promise.all(tasks);
}

async function preloadRestoredHistory(context: PreloadContext): Promise<void> {
  const owner = context.stores.sessionStore.currentSelectedItem;
  const topicId = context.stores.sessionStore.currentTopicId;
  if (
    !owner?.id ||
    !topicId ||
    (owner.type !== "agent" && owner.type !== "group")
  ) {
    return;
  }
  console.log(
    `[Lifecycle] Preloading chat history for ${owner.type} ${owner.id}, topic: ${topicId}`,
  );
  await useChatHistoryStore().preloadHistory(owner.id, owner.type, topicId, 5);
}

async function startPreloading(context: PreloadContext): Promise<void> {
  const currentState = context.getState();
  if (currentState === "PRELOADING" || currentState === "READY") {
    console.log(`[Lifecycle] Skip preloading in state: ${currentState}`);
    return;
  }
  context.cleanupConnectionWaiters();
  context.setState("PRELOADING", "开始预加载核心业务数据");
  const startTime = Date.now();
  try {
    context.updatePhaseLabel("正在并发预加载配置与助手数据...");
    await preloadDependencies(context);
    console.log(
      `[Lifecycle] [Concurrent] DONE Preloading in ${Date.now() - startTime}ms`,
    );
    context.updatePhaseLabel("核心数据预加载完成");
    await preloadRestoredHistory(context);
    context.markReady();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    context.fail(`预加载失败: ${message}`);
    throw error;
  }
}

export function createPreloadActions(context: PreloadContext): PreloadActions {
  return { startPreloading: () => startPreloading(context) };
}
