import { invoke } from "@tauri-apps/api/core";
import type {
  DistributedToolConfigStatus,
} from "./useDistributedAuthorization";
import type {
  InvocationCommand,
  PluginItem,
  PlaceholderItem,
  RawInvocationCommand,
} from "./distributedToolTypes";
import type { DistributedToolState } from "./distributedToolState";

export interface DistributedAuthorizationReader {
  readConfig: () => Promise<DistributedToolConfigStatus>;
  currentMutationVersion: () => number;
  hasPendingMutation?: () => boolean;
  waitForMutations?: () => Promise<void>;
}

let metadataRequest = 0;

function optionalString(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  return trimmed || undefined;
}

export function normalizeInvocationCommands(tool: Record<string, any>): InvocationCommand[] {
  const rawCommands =
    tool.capabilities?.invocationCommands || tool.invocation_commands || [];
  if (!Array.isArray(rawCommands)) {
    console.warn("[DistributedTools] invocationCommands 格式无效：", tool.name);
    return [];
  }
  return rawCommands.map((rawCommand, index) => {
    const command =
      rawCommand && typeof rawCommand === "object"
        ? (rawCommand as RawInvocationCommand)
        : {};
    const commandIdentifier =
      optionalString(command.commandIdentifier) ||
      optionalString(command.command_identifier) ||
      null;
    if (!commandIdentifier) {
      console.warn(
        `[DistributedTools] 工具 ${tool.name || "未知"} 的第 ${index + 1} 个命令缺少标识。`,
      );
    }
    return {
      commandIdentifier,
      description: optionalString(command.description),
      example: optionalString(command.example),
      missingIdentifier: !commandIdentifier,
    };
  });
}

function enabledFromConfig(
  name: string,
  metadataEnabled: boolean,
  config: DistributedToolConfigStatus,
): boolean {
  if (config.enabledNames) return config.enabledNames.includes(name);
  if (config.disabledNames) return !config.disabledNames.includes(name);
  return metadataEnabled;
}

function normalizePlugin(tool: Record<string, any>, enabled: boolean): PluginItem {
  const category = tool.category;
  const type =
    category === "interactive" || category === "streaming" ? category : "oneshot";
  return {
    id: optionalString(tool.name) || "unknown",
    name: optionalString(tool.display_name) || optionalString(tool.name) || "未命名工具",
    englishName: optionalString(tool.name) || "unknown",
    description: optionalString(tool.description) || "",
    type,
    placeholder: optionalString(tool.placeholder),
    invocationCommands: normalizeInvocationCommands(tool),
    icon: optionalString(tool.icon) || "i-lucide-toy-brick",
    communication: tool.communication,
    enabled,
    requiresRoot: tool.requiresRoot === true || tool.requires_root === true,
  };
}

function normalizePlaceholders(tools: Record<string, any>[]): PlaceholderItem[] {
  return tools.flatMap((tool) => {
    const macro = optionalString(tool.placeholder);
    if (!macro) return [];
    const name = optionalString(tool.display_name) || optionalString(tool.name) || "工具";
    return [
      {
        macro,
        name: `${name}占位宏`,
        description: `解析并流式替换为 ${name} 的最新物理遥测采样。`,
        example: "点击插件卡片内的显式按钮读取实时数据。",
      },
    ];
  });
}

export async function loadDistributedToolMetadata(
  authorization: DistributedAuthorizationReader,
  state: DistributedToolState,
): Promise<void> {
  const requestId = ++metadataRequest;
  try {
    for (let attempt = 0; attempt < 8; attempt += 1) {
      await authorization.waitForMutations?.();
      if (requestId !== metadataRequest) return;
      const mutationAtStart = authorization.currentMutationVersion();
      const [rawResult, configResult] = await Promise.all([
        invoke<unknown>("get_registered_tools_metadata"),
        authorization.readConfig(),
      ]);
      if (requestId !== metadataRequest) return;
      if (
        mutationAtStart !== authorization.currentMutationVersion() ||
        authorization.hasPendingMutation?.()
      ) {
        continue;
      }
      if (!Array.isArray(rawResult)) throw new Error("工具元数据格式错误");
      const rawTools = rawResult.filter(
        (tool): tool is Record<string, any> => !!tool && typeof tool === "object",
      );
      state.pluginsList.value = rawTools.map((tool) =>
        normalizePlugin(
          tool,
          enabledFromConfig(tool.name, tool.enabled !== false, configResult),
        ),
      );
      state.placeholdersList.value = normalizePlaceholders(rawTools);
      return;
    }
    console.warn("[DistributedTools] 授权写入持续进行，忽略过期工具元数据");
  } catch (error) {
    if (requestId === metadataRequest) {
      state.pluginsList.value = [];
      state.placeholdersList.value = [];
    }
    console.error("[DistributedTools] 加载工具授权状态失败，已安全关闭工具：", error);
    throw error;
  }
}
