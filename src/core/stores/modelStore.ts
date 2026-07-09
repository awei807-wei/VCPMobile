import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { invoke, Channel } from "@tauri-apps/api/core";
import { useNotificationStore } from "./notification";
import { useConnectionSwitchGuardStore } from "./connectionSwitchGuard";

export interface ModelInfo {
  id: string;
  object: string;
  created: number;
  owned_by: string;
}

export interface TestResult {
  status: "idle" | "testing" | "success" | "failed";
  latency?: number;
  error?: string;
}

export const useModelStore = defineStore("model", () => {
  // --- State ---
  const models = ref<ModelInfo[]>([]);
  const hotModels = ref<string[]>([]);
  const favorites = ref<string[]>([]);
  const isLoading = ref(false);
  const lastRefreshed = ref(0);
  const forceNextRefresh = ref(false);
  const cacheGeneration = ref(0);
  const testResults = ref<Record<string, TestResult>>({});

  const notificationStore = useNotificationStore();
  const switchGuardStore = useConnectionSwitchGuardStore();

  // --- Getters ---
  const sortedModels = computed(() => {
    // 排序优先级：收藏 > 热门 > 其他
    return [...models.value].sort((a, b) => {
      const aFav = favorites.value.includes(a.id) ? 1 : 0;
      const bFav = favorites.value.includes(b.id) ? 1 : 0;
      if (aFav !== bFav) return bFav - aFav;

      const aHot = hotModels.value.indexOf(a.id);
      const bHot = hotModels.value.indexOf(b.id);
      if (aHot !== -1 && bHot !== -1) return aHot - bHot;
      if (aHot !== -1) return -1;
      if (bHot !== -1) return 1;

      return a.id.localeCompare(b.id);
    });
  });

  const isFavorite = computed(
    () => (modelId: string) => favorites.value.includes(modelId)
  );

  // --- Actions ---
  const fetchModels = async (force = false) => {
    if (switchGuardStore.switching) return;
    if (isLoading.value) return; // 锁频防护，防止并发刷请求

    const shouldRefresh = force || forceNextRefresh.value;

    if (
      !shouldRefresh &&
      models.value.length > 0 &&
      Date.now() - lastRefreshed.value < 1000 * 60 * 10
    ) {
      return;
    }

    const startTime = Date.now();
    const generationAtStart = cacheGeneration.value;
    isLoading.value = true;
    try {
      // 先尝试获取缓存
      if (!shouldRefresh && models.value.length === 0) {
        const cachedModels = await invoke<ModelInfo[]>("get_cached_models");
        if (generationAtStart !== cacheGeneration.value) return;
        models.value = cachedModels;
      }

      // 只有在明确要求或没缓存时才刷新
      if (shouldRefresh || models.value.length === 0) {
        const refreshedModels = await invoke<ModelInfo[]>("refresh_models");
        if (generationAtStart !== cacheGeneration.value) return;
        models.value = refreshedModels;
        lastRefreshed.value = Date.now();
        forceNextRefresh.value = false;

        if (force) {
          notificationStore.addNotification({
            type: "success",
            title: "模型同步成功",
            message: `已成功同步最新模型列表，共 ${models.value.length} 个可用模型`,
            toastOnly: true,
          });
        }
      }

      await Promise.all([fetchHotModels(), fetchFavorites()]);
    } catch (error: any) {
      console.error("Failed to fetch models:", error);
      if (force) {
        notificationStore.addNotification({
          type: "error",
          title: "模型同步失败",
          message:
            error?.toString() || "请检查网络连接、API 服务器或 API 密钥配置",
          toastOnly: true,
        });
      }
    } finally {
      // 转圈动画平滑停止延迟机制
      if (force) {
        const elapsed = Date.now() - startTime;
        const minDuration = 800; // 最低转圈时长保证 800ms
        if (elapsed < minDuration) {
          await new Promise((resolve) =>
            setTimeout(resolve, minDuration - elapsed)
          );
        }
      }
      isLoading.value = false;
    }
  };

  const fetchHotModels = async () => {
    try {
      hotModels.value = await invoke<string[]>("get_hot_models", { limit: 10 });
    } catch (error) {
      console.error("Failed to fetch hot models:", error);
    }
  };

  const fetchFavorites = async () => {
    try {
      favorites.value = await invoke<string[]>("get_favorite_models");
    } catch (error) {
      console.error("Failed to fetch favorite models:", error);
    }
  };

  const toggleFavorite = async (modelId: string) => {
    try {
      const isFav = await invoke<boolean>("toggle_favorite_model", { modelId });
      if (isFav) {
        if (!favorites.value.includes(modelId)) favorites.value.push(modelId);
      } else {
        favorites.value = favorites.value.filter((id) => id !== modelId);
      }
    } catch (error) {
      console.error("Failed to toggle favorite:", error);
    }
  };

  const recordUsage = async (modelId: string) => {
    try {
      await invoke("record_model_usage", { modelId });
      // 更新本地热门列表（可选，或者等待下次 fetch）
      fetchHotModels();
    } catch (error) {
      console.error("Failed to record usage:", error);
    }
  };

  // --- Cleanup helper for stale test results ---
  const cleanupOldTestResults = () => {
    const activeModelIds = new Set(models.value.map((m) => m.id));
    for (const modelId in testResults.value) {
      if (!activeModelIds.has(modelId)) {
        delete testResults.value[modelId];
      }
    }
  };

  // --- Silent background sync ---
  const triggerSilentSync = async () => {
    try {
      const freshModels = await invoke<ModelInfo[]>("refresh_models");
      models.value = freshModels;
      lastRefreshed.value = Date.now();
      await Promise.all([fetchHotModels(), fetchFavorites()]);
      cleanupOldTestResults();
      console.log(
        `[SWR/Self-Healing] Silent sync completed. Total models: ${freshModels.length}`
      );
    } catch (error) {
      console.warn("[SWR/Self-Healing] Silent sync failed:", error);
    }
  };

  // --- Batch Model Testing ---
  const isTestingAll = ref(false);
  const activeSessionId = ref(0);

  const stopTestAll = async () => {
    isTestingAll.value = false;
    activeSessionId.value++;

    try {
      await invoke("stop_all_model_tests");
    } catch (e) {
      console.error("[ModelStore] Failed to stop model tests on backend:", e);
    }

    for (const modelId in testResults.value) {
      if (testResults.value[modelId]?.status === "testing") {
        delete testResults.value[modelId];
      }
    }
  };

  const testModel = async (modelId: string) => {
    const sessionId = activeSessionId.value;
    testResults.value[modelId] = { status: "testing" };
    try {
      const latency = await invoke<number>("test_model_connectivity", {
        modelId,
      });
      if (activeSessionId.value === sessionId) {
        testResults.value[modelId] = { status: "success", latency };
      } else {
        delete testResults.value[modelId];
      }
    } catch (error: any) {
      console.error(`Failed to test connectivity for ${modelId}:`, error);
      const errStr = error?.toString() || "连接失败";
      if (activeSessionId.value === sessionId) {
        testResults.value[modelId] = { status: "failed", error: errStr };
        if (errStr.includes("404")) {
          models.value = models.value.filter((m) => m.id !== modelId);
          triggerSilentSync();
        }
      } else {
        delete testResults.value[modelId];
      }
    }
  };

  const testAllModels = async (modelIds: string[]) => {
    if (isTestingAll.value) return;

    const targets = modelIds.filter(
      (id) =>
        !testResults.value[id] || testResults.value[id].status !== "testing"
    );
    if (targets.length === 0) return;

    isTestingAll.value = true;

    try {
      const progressChannel = new Channel<any>();
      progressChannel.onmessage = (progress: {
        modelId: string;
        status: "testing" | "success" | "failed" | "completed";
        latency?: number;
        error?: string;
      }) => {
        const { modelId, status, latency, error } = progress;

        if (status === "completed") {
          isTestingAll.value = false;
          return;
        }

        if (status === "testing") {
          testResults.value[modelId] = { status: "testing" };
        } else if (status === "success") {
          testResults.value[modelId] = { status: "success", latency };
        } else if (status === "failed") {
          testResults.value[modelId] = {
            status: "failed",
            error: error || "连接失败",
          };
          if (error?.includes("404")) {
            models.value = models.value.filter((m) => m.id !== modelId);
            triggerSilentSync();
          }
        }
      };

      await invoke("start_batch_model_test", {
        modelIds: targets,
        progressChannel,
      });
    } catch (e) {
      console.error("[ModelStore] Failed to start batch model test:", e);
      isTestingAll.value = false;
    }
  };

  const markModelsStale = () => {
    cacheGeneration.value += 1;
    models.value = [];
    lastRefreshed.value = 0;
    forceNextRefresh.value = true;
  };

  const invalidatePersistedCache = async () => {
    markModelsStale();
    await invoke("invalidate_model_cache");
  };

  return {
    models,
    hotModels,
    favorites,
    isLoading,
    lastRefreshed,
    testResults,
    isTestingAll,
    sortedModels,
    isFavorite,
    fetchModels,
    fetchHotModels,
    fetchFavorites,
    toggleFavorite,
    recordUsage,
    markModelsStale,
    invalidatePersistedCache,
    testModel,
    testAllModels,
    stopTestAll,
  };
});
