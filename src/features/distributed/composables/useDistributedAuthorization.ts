import { computed, readonly, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

export interface DistributedToolConfigStatus {
  enabledNames: string[] | null;
  disabledNames: string[] | null;
}

let mutationTail: Promise<unknown> = Promise.resolve();
let mutationVersion = 0;
const pendingMutationCount = ref(0);

/** 所有授权写入共享同一个计数，避免不同组件各自提前解除 loading。 */
export const distributedAuthorizationPendingCount =
  readonly(pendingMutationCount);

function stringList(value: unknown): string[] | null {
  if (!Array.isArray(value)) return null;
  const names = value.filter(
    (item): item is string => typeof item === "string",
  );
  return names.length === value.length ? names : null;
}

function readStringList(
  raw: Record<string, unknown>,
  camelName: string,
  snakeName: string,
): string[] | null {
  const value = raw[camelName] ?? raw[snakeName];
  if (value === undefined || value === null) return null;
  const names = stringList(value);
  if (names === null)
    throw new Error(`分布式工具授权字段 ${camelName} 格式错误`);
  return names;
}

export function parseToolConfigStatus(
  value: unknown,
): DistributedToolConfigStatus {
  if (!value || typeof value !== "object") {
    throw new Error("分布式工具授权状态格式错误");
  }
  const raw = value as Record<string, unknown>;
  const enabledNames = readStringList(raw, "enabledNames", "enabled_names");
  const disabledNames = readStringList(raw, "disabledNames", "disabled_names");
  if (enabledNames === null && disabledNames === null) {
    throw new Error("分布式工具授权状态缺少 enabledNames/disabledNames");
  }
  if (enabledNames !== null && disabledNames !== null) {
    throw new Error("分布式工具授权状态同时包含两种互斥配置");
  }
  return { enabledNames, disabledNames };
}

function enqueueMutation<T>(operation: () => Promise<T>): Promise<T> {
  const run = mutationTail.catch(() => undefined).then(operation);
  mutationTail = run.catch(() => undefined);
  return run;
}

function beginMutation(): void {
  mutationVersion += 1;
  pendingMutationCount.value += 1;
}

function endMutation(): void {
  pendingMutationCount.value = Math.max(0, pendingMutationCount.value - 1);
}

export function useDistributedAuthorization() {
  const loading = computed(() => pendingMutationCount.value > 0);

  async function readConfig(): Promise<DistributedToolConfigStatus> {
    const result = await invoke("get_distributed_tool_config_status");
    return parseToolConfigStatus(result);
  }

  async function updateEnabledTools(enabledNames: string[]): Promise<void> {
    const uniqueNames = [...new Set(enabledNames)].filter(Boolean);
    beginMutation();
    try {
      await enqueueMutation(() =>
        invoke("update_enabled_tools", { enabledNames: uniqueNames }).then(
          () => undefined,
        ),
      );
    } finally {
      endMutation();
    }
  }

  async function resetDisabledTools(): Promise<void> {
    beginMutation();
    try {
      await enqueueMutation(() =>
        invoke("reset_distributed_tools_disabled").then(() => undefined),
      );
    } finally {
      endMutation();
    }
  }

  function currentMutationVersion(): number {
    return mutationVersion;
  }

  function hasPendingMutation(): boolean {
    return pendingMutationCount.value > 0;
  }

  function waitForMutations(): Promise<void> {
    return mutationTail.then(() => undefined);
  }

  return {
    loading,
    pendingCount: distributedAuthorizationPendingCount,
    readConfig,
    updateEnabledTools,
    resetDisabledTools,
    currentMutationVersion,
    hasPendingMutation,
    waitForMutations,
  };
}
