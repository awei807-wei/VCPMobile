import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useConnectionProfilesStore } from "@/core/stores/connectionProfiles";
import { useSettingsStore } from "@/core/stores/settings";
import { useSyncSessionStore } from "@/core/stores/syncSession";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";

const profile = (id: "lan" | "wan") => ({
  id,
  name: id === "lan" ? "内网" : "外网",
  vcpServerUrl: "https://vcp.example.test",
  vcpApiKey: "vcp-key",
  vcpLogUrl: "wss://log.example.test",
  vcpLogKey: "log-key",
  syncServerUrl: "ws://sync.example.test",
  syncHttpUrl: "http://sync.example.test",
  syncToken: "sync-token",
  distributedWsUrl: "wss://log.example.test",
  distributedVcpKey: "log-key",
});

describe("connection profile switch and sync ownership", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("plugin:vcp-mobile|get_battery_status", () => ({
      level: 90,
      isPowerSaveMode: false,
    }));
    mockInvoke("start_manual_sync", () => 11);
    mockInvoke("is_assistant_chat_active", () => false);
    mockInvoke("is_sync_active", () => true);
    mockInvoke("stop_sync", () => undefined);
    mockInvoke("invalidate_model_cache", () => undefined);
  });

  it("allows switching during sync only after awaiting stop", async () => {
    let releaseStop!: () => void;
    const stopPromise = new Promise<void>((resolve) => {
      releaseStop = resolve;
    });
    mockInvoke("stop_sync", () => stopPromise);
    const settings = useSettingsStore();
    const current = {
      activeConnectionProfileId: "lan",
      connectionProfiles: [profile("lan"), profile("wan")],
    } as any;
    settings.settings = current;
    mockInvoke("update_settings", (args) => ({
      ...current,
      ...((args as Record<string, any> | undefined)?.updates ?? {}),
    }));
    const sync = useSyncSessionStore();
    sync.open();
    await sync.startSync();
    expect(sync.isActive).toBe(true);

    const profiles = useConnectionProfilesStore();
    const switching = profiles.switchTo("wan");
    await vi.waitFor(() =>
      expect(
        invokeMock.mock.calls.some(([command]) => command === "is_sync_active"),
      ).toBe(true),
    );
    expect(
      invokeMock.mock.calls.some(([command]) => command === "stop_sync"),
    ).toBe(true);
    expect(
      invokeMock.mock.calls.some(([command]) => command === "update_settings"),
    ).toBe(false);

    releaseStop();
    await switching;
    const commands = invokeMock.mock.calls.map(([command]) => command);
    expect(commands.indexOf("stop_sync")).toBeLessThan(
      commands.indexOf("update_settings"),
    );
    expect(settings.settings?.activeConnectionProfileId).toBe("wan");
  });
});
