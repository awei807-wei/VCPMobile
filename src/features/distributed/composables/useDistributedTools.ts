import { computed } from "vue";
import { useNotificationStore } from "../../../core/stores/notification";
import { useDistributedAuthorization } from "./useDistributedAuthorization";
import { distributedToolState } from "./distributedToolState";
import {
  loadDistributedToolMetadata,
  normalizeInvocationCommands,
} from "./useDistributedToolMetadata";
import {
  resetDistributedTools,
  distributedResetPending,
  distributedToolPendingCount,
  isDistributedToolMutationPending,
  setDistributedToolEnabled,
} from "./useDistributedToolAuthorization";
import {
  loadDistributedPluginDetails,
  parseFoldBlocks,
  selectDistributedFoldBlock,
} from "./useDistributedToolDetails";

export type {
  FoldBlock,
  InvocationCommand,
  PlaceholderItem,
  PluginItem,
  RawInvocationCommand,
} from "./distributedToolTypes";
export { normalizeInvocationCommands, parseFoldBlocks };

export function useDistributedTools() {
  const authorization = useDistributedAuthorization();
  const notificationStore = useNotificationStore();
  const authorizationLoading = computed(
    () =>
      authorization.loading.value ||
      distributedToolPendingCount.value > 0 ||
      distributedResetPending.value,
  );
  const loadPluginsMetadata = () =>
    loadDistributedToolMetadata(authorization, distributedToolState);
  const setToolEnabled = (
    plugin: import("./distributedToolTypes").PluginItem,
    targetState: boolean,
  ) =>
    setDistributedToolEnabled(
      authorization,
      distributedToolState,
      notificationStore,
      plugin,
      targetState,
    );
  const resetDisabledTools = () =>
    resetDistributedTools(
      authorization,
      notificationStore,
      loadPluginsMetadata,
    );
  const loadPluginDetails = (
    plugin: import("./distributedToolTypes").PluginItem,
  ) => loadDistributedPluginDetails(distributedToolState, plugin);
  const selectFoldBlock = (pluginId: string, index: number) =>
    selectDistributedFoldBlock(distributedToolState, pluginId, index);
  const clear = () => {
    distributedToolState.pluginsList.value = [];
    distributedToolState.placeholdersList.value = [];
    distributedToolState.pluginLoading.value = {};
    distributedToolState.pluginData.value = {};
    distributedToolState.pluginFoldBlocks.value = {};
    distributedToolState.selectedFoldBlockIdx.value = {};
    distributedToolState.pluginDetailRequestEpoch.value = {};
  };

  return {
    ...distributedToolState,
    authorizationLoading,
    authorizationPendingCount: authorization.pendingCount,
    authorizationResetPending: distributedResetPending,
    isToolMutationPending: isDistributedToolMutationPending,
    loadPluginsMetadata,
    setToolEnabled,
    resetDisabledTools,
    loadPluginDetails,
    selectFoldBlock,
    clear,
  };
}
