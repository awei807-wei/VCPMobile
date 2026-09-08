import { ref } from "vue";
import type {
  FoldBlock,
  PlaceholderItem,
  PluginItem,
} from "./distributedToolTypes";

export const distributedToolState = {
  pluginsList: ref<PluginItem[]>([]),
  placeholdersList: ref<PlaceholderItem[]>([]),
  pluginLoading: ref<Record<string, boolean>>({}),
  pluginData: ref<Record<string, string>>({}),
  pluginFoldBlocks: ref<Record<string, FoldBlock[]>>({}),
  selectedFoldBlockIdx: ref<Record<string, number>>({}),
  pluginDetailRequestEpoch: ref<Record<string, number>>({}),
  pluginDetailRequestSequence: ref(0),
};

export type DistributedToolState = typeof distributedToolState;
