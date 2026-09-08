import { beforeEach, describe, expect, it, vi } from "vitest";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("分布式工具授权契约", () => {
  beforeEach(() => {
    invokeMock.mockClear();
  });

  it("配置状态缺少完整 allowlist 时安全拒绝", async () => {
    const { parseToolConfigStatus } =
      await import("@/features/distributed/composables/useDistributedAuthorization");
    expect(() => parseToolConfigStatus({ loaded: true })).toThrow(
      "缺少 enabledNames/disabledNames",
    );
    expect(() => parseToolConfigStatus({ enabledNames: ["A", 3] })).toThrow();
  });

  it("兼容 Rust 的 snake_case 状态并保留空 allowlist 的权威语义", async () => {
    const { parseToolConfigStatus } =
      await import("@/features/distributed/composables/useDistributedAuthorization");
    expect(
      parseToolConfigStatus({
        loaded: false,
        enabled_names: [],
        source: "default",
      }),
    ).toEqual({ enabledNames: [], disabledNames: null });
  });

  it("所有授权写入通过单一 promise tail 串行执行", async () => {
    const first = deferred<void>();
    mockInvoke("update_enabled_tools", () => first.promise);
    mockInvoke("reset_distributed_tools_disabled", () => undefined);
    const { useDistributedAuthorization } =
      await import("@/features/distributed/composables/useDistributedAuthorization");
    const authorization = useDistributedAuthorization();
    const updating = authorization.updateEnabledTools(["A", "A", ""]);
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("update_enabled_tools", {
        enabledNames: ["A"],
      }),
    );
    expect(authorization.pendingCount.value).toBe(1);
    expect(authorization.loading.value).toBe(true);
    const resetting = authorization.resetDisabledTools();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    first.resolve();
    await Promise.all([updating, resetting]);
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "update_enabled_tools",
      "reset_distributed_tools_disabled",
    ]);
    expect(authorization.pendingCount.value).toBe(0);
    expect(authorization.loading.value).toBe(false);
  });

  it("授权写入挂起时旧元数据不会覆盖，写入完成后重新读取", async () => {
    const oldMetadata = deferred<unknown>();
    const oldConfig = deferred<unknown>();
    const update = deferred<void>();
    let metadataCalls = 0;
    let configCalls = 0;
    mockInvoke("get_registered_tools_metadata", () => {
      metadataCalls += 1;
      return metadataCalls === 1
        ? oldMetadata.promise
        : [{ name: "MobileCPU", category: "streaming", enabled: true }];
    });
    mockInvoke("get_distributed_tool_config_status", () => {
      configCalls += 1;
      return configCalls === 1
        ? oldConfig.promise
        : { enabled_names: ["MobileCPU"] };
    });
    mockInvoke("update_enabled_tools", () => update.promise);

    const { useDistributedAuthorization } =
      await import("@/features/distributed/composables/useDistributedAuthorization");
    const { distributedToolState } =
      await import("@/features/distributed/composables/distributedToolState");
    const { loadDistributedToolMetadata } =
      await import("@/features/distributed/composables/useDistributedToolMetadata");
    distributedToolState.pluginsList.value = [];
    distributedToolState.placeholdersList.value = [];
    const authorization = useDistributedAuthorization();
    const loading = loadDistributedToolMetadata(
      authorization,
      distributedToolState,
    );
    await vi.waitFor(() => expect(metadataCalls).toBe(1));

    const updating = authorization.updateEnabledTools(["MobileCPU"]);
    oldMetadata.resolve([
      { name: "MobileCPU", category: "streaming", enabled: false },
    ]);
    oldConfig.resolve({ disabled_names: ["MobileCPU"] });
    await Promise.resolve();
    expect(distributedToolState.pluginsList.value).toHaveLength(0);

    update.resolve();
    await Promise.all([loading, updating]);
    expect(distributedToolState.pluginsList.value[0]).toMatchObject({
      id: "MobileCPU",
      enabled: true,
    });
    expect(metadataCalls).toBe(2);
    expect(configCalls).toBe(2);
  });
});
