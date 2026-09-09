import { invoke } from "@tauri-apps/api/core";
import type { DistributedToolState } from "./distributedToolState";
import type { FoldBlock, PluginItem } from "./distributedToolTypes";

export function parseFoldBlocks(raw: string): FoldBlock[] {
  if (!raw.includes("[===vcp_fold:")) return [];
  const regex =
    /\[===vcp_fold:\s*([\d.]+)(?:\s*::desc:\s*([^\]]+))?===\]\n?([\s\S]*?)(?=\n?\[===vcp_fold:|$)/g;
  const blocks: FoldBlock[] = [];
  let match: RegExpExecArray | null;
  while ((match = regex.exec(raw)) !== null) {
    const threshold = Number.parseFloat(match[1]);
    if (!Number.isFinite(threshold)) continue;
    blocks.push({
      threshold,
      desc: match[2]?.trim() || `层级 ${threshold}`,
      content: match[3].trim(),
    });
  }
  return blocks;
}

function formatInvocationGuide(plugin: PluginItem): string {
  const header = [
    `${plugin.englishName} 是 ${plugin.type} 工具。`,
    "该类型不提供实时物理遥测快照，需要由分布式主服务器按 manifest 参数发起调用。",
  ];
  if (!plugin.invocationCommands.length) return header.join("\n");
  const body = plugin.invocationCommands.map((command, index) =>
    [
      `命令 ${index + 1}: ${command.commandIdentifier || "未声明命令标识"}`,
      command.missingIdentifier
        ? "配置问题：manifest 缺少 commandIdentifier 或 command_identifier。"
        : "",
      command.description ? `说明：\n${command.description}` : "",
      command.example ? `示例：\n${command.example}` : "",
    ]
      .filter(Boolean)
      .join("\n"),
  );
  return [...header, "", ...body].join("\n\n");
}

function storeJsonResult(
  state: DistributedToolState,
  plugin: PluginItem,
  result: string,
): void {
  try {
    const parsed = JSON.parse(result);
    state.pluginData.value[plugin.id] =
      parsed && typeof parsed === "object"
        ? JSON.stringify(parsed, null, 2)
        : result;
  } catch {
    state.pluginData.value[plugin.id] = result;
  }
}

function nextRequestEpoch(
  state: DistributedToolState,
  pluginId: string,
): number {
  const next = state.pluginDetailRequestSequence.value + 1;
  state.pluginDetailRequestSequence.value = next;
  state.pluginDetailRequestEpoch.value[pluginId] = next;
  return next;
}

function isCurrentRequest(
  state: DistributedToolState,
  pluginId: string,
  epoch: number,
): boolean {
  return state.pluginDetailRequestEpoch.value[pluginId] === epoch;
}

export async function loadDistributedPluginDetails(
  state: DistributedToolState,
  plugin: PluginItem,
): Promise<void> {
  const requestEpoch = nextRequestEpoch(state, plugin.id);
  state.pluginLoading.value[plugin.id] = true;
  state.pluginFoldBlocks.value[plugin.id] = [];
  state.selectedFoldBlockIdx.value[plugin.id] = 0;
  try {
    if (plugin.type !== "streaming") {
      if (isCurrentRequest(state, plugin.id, requestEpoch)) {
        state.pluginData.value[plugin.id] = formatInvocationGuide(plugin);
      }
      return;
    }
    const result = await invoke<string>("execute_distributed_tool", {
      name: plugin.id,
    });
    if (!isCurrentRequest(state, plugin.id, requestEpoch)) return;
    const blocks = parseFoldBlocks(result);
    if (blocks.length) {
      state.pluginFoldBlocks.value[plugin.id] = blocks;
      state.pluginData.value[plugin.id] = blocks[0].content;
    } else {
      storeJsonResult(state, plugin, result);
    }
  } catch (error) {
    if (!isCurrentRequest(state, plugin.id, requestEpoch)) return;
    console.error(`[DistributedTools] 读取工具 ${plugin.id} 失败：`, error);
    state.pluginData.value[plugin.id] =
      `遥测读取异常：\n${String(error)}\n请检查设备权限与传感器状态。`;
  } finally {
    if (isCurrentRequest(state, plugin.id, requestEpoch)) {
      state.pluginLoading.value[plugin.id] = false;
    }
  }
}

export function selectDistributedFoldBlock(
  state: DistributedToolState,
  pluginId: string,
  index: number,
): void {
  state.selectedFoldBlockIdx.value[pluginId] = index;
  const block = state.pluginFoldBlocks.value[pluginId]?.[index];
  if (block) state.pluginData.value[pluginId] = block.content;
}
