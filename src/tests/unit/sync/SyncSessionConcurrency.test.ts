import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mount } from "@vue/test-utils";
import { useSyncSessionStore } from "@/core/stores/syncSession";
import type { SyncTerminalError } from "@/core/stores/syncSession/types";
import { useSettingsStore } from "@/core/stores/settings";
import SyncSessionView from "@/features/sync/SyncSessionView.vue";
import SyncLogBrowserCore from "@/features/settings/components/SyncLogBrowserCore.vue";
import {
  emitTauriEvent,
  invokeMock,
  listenMock,
  mockInvoke,
} from "@/tests/mocks/tauri";

const validSummary = (warnings = 0) => ({
  successfulTopics: 1,
  totalTopics: 1,
  failedTopics: 0,
  legacyAttachmentWarnings: warnings,
  failedTopicIds: [],
});

const validError = (
  retryAction: "manual" | "after_user_action" | "never" = "manual",
): SyncTerminalError => ({
  code: "SYNC_FAILED",
  category: "data",
  origin: "desktop_plugin",
  stage: "messages",
  retryAction,
  message: "同步未能完成",
  guidance: "请检查两端配置后重试。",
  failedTopicIds: [],
  logFile: "sync.log",
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe("sync session event ownership", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("stop_sync", () => undefined);
    mockInvoke("plugin:vcp-mobile|get_battery_status", () => ({
      level: 80,
      isPowerSaveMode: false,
    }));
    mockInvoke("start_manual_sync", () => 1);
    mockInvoke("list_sync_log_files", () => []);
  });

  it("registers listeners before invoking start_manual_sync", async () => {
    const registration = deferred<() => void>();
    listenMock.mockImplementationOnce(() => registration.promise);
    const store = useSyncSessionStore();
    store.open();
    const starting = store.startSync();
    await Promise.resolve();
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "start_manual_sync",
      ),
    ).toBe(false);
    registration.resolve(vi.fn());
    await starting;
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "start_manual_sync",
      ),
    ).toBe(true);
  });

  it("buffers session events emitted before start_manual_sync returns", async () => {
    const start = deferred<number>();
    mockInvoke("start_manual_sync", () => start.promise);
    const store = useSyncSessionStore();
    store.open();
    const starting = store.startSync();
    await vi.waitFor(() =>
      expect(
        invokeMock.mock.calls.some(
          ([command]) => command === "start_manual_sync",
        ),
      ).toBe(true),
    );
    emitTauriEvent("vcp-sync-status", {
      sessionId: 7,
      attemptId: 0,
      status: "open",
    });
    expect(store.status).toBe("connecting");
    start.resolve(7);
    await starting;
    expect(store.activeSessionId).toBe(7);
    expect(store.status).toBe("connected");
  });

  it("ignores stale sessions and attempts", async () => {
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      attemptId: 1,
      status: "open",
    });
    emitTauriEvent("vcp-sync-progress", {
      sessionId: 1,
      attemptId: 1,
      phase: "messages",
      total: 2,
      completed: 1,
    });
    emitTauriEvent("vcp-sync-progress", {
      sessionId: 1,
      attemptId: 0,
      phase: "messages",
      total: 2,
      completed: 0,
    });
    emitTauriEvent("vcp-sync-progress", {
      sessionId: 99,
      attemptId: 2,
      phase: "messages",
      total: 2,
      completed: 0,
    });
    expect(store.progressData.completed).toBe(1);
  });

  it("rejects a late event after the profile epoch changes", async () => {
    const settings = useSettingsStore();
    const activeSettings = { activeConnectionProfileId: "lan" } as any;
    settings.settings = activeSettings;
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      attemptId: 1,
      status: "open",
    });
    activeSettings.activeConnectionProfileId = "wan";
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      attemptId: 1,
      status: "error",
      error: validError(),
    });
    expect(store.status).toBe("connected");
  });

  it("does not stack listeners across close and reopen", async () => {
    const store = useSyncSessionStore();
    store.open();
    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(4));
    await store.close();
    store.open();
    await vi.waitFor(() => expect(listenMock).toHaveBeenCalledTimes(8));
    await store.startSync();
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      attemptId: 1,
      status: "open",
    });
    expect(store.status).toBe("connected");
  });

  it("awaits stop before retrying", async () => {
    let nextSession = 0;
    const stopping = deferred<void>();
    mockInvoke("start_manual_sync", () => ++nextSession);
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      attemptId: 1,
      status: "error",
      error: validError(),
    });
    mockInvoke("stop_sync", () => stopping.promise);
    const retrying = store.retrySync();
    await vi.waitFor(() =>
      expect(
        invokeMock.mock.calls.some(([command]) => command === "stop_sync"),
      ).toBe(true),
    );
    expect(
      invokeMock.mock.calls.some(
        ([command]) => command === "start_manual_sync",
      ),
    ).toBe(true);
    stopping.resolve();
    await retrying;
    expect(store.activeSessionId).toBe(2);
  });

  it("uses structured errors and only offers the declared retry action", async () => {
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent("vcp-sync-status", {
      sessionId: 1,
      attemptId: 1,
      status: "error",
      error: validError("never"),
    });
    expect(store.terminalError?.retryAction).toBe("never");
    const wrapper = mount(SyncSessionView);
    expect(wrapper.text()).not.toContain("重新同步");
  });

  it("exposes exactly one reachable stop or retry action", async () => {
    const store = useSyncSessionStore();
    store.open();
    store.status = "connected";
    store.canDismiss = false;
    const wrapper = mount(SyncSessionView);

    const stopButtons = wrapper
      .findAll("button")
      .filter((button) => button.text().includes("停止同步"));
    expect(stopButtons).toHaveLength(1);
    await stopButtons[0].trigger("click");
    expect(
      invokeMock.mock.calls.some(([command]) => command === "stop_sync"),
    ).toBe(true);

    store.status = "error";
    store.terminalError = validError("manual");
    await wrapper.vm.$nextTick();
    const retryButtons = wrapper
      .findAll("button")
      .filter((button) => button.text().includes("重新同步"));
    expect(retryButtons).toHaveLength(1);
  });

  it("accepts only a validated completion summary and exposes warnings", async () => {
    const store = useSyncSessionStore();
    store.open();
    await store.startSync();
    emitTauriEvent("vcp-sync-completed", {
      sessionId: 1,
      attemptId: 1,
      status: "completed_with_warnings",
      summary: validSummary(2),
    });
    expect(store.status).toBe("completed_with_warnings");
    expect(store.summary.legacyAttachmentWarnings).toBe(2);
    emitTauriEvent("vcp-sync-completed", {
      sessionId: 1,
      attemptId: 1,
      status: "completed",
      summary: { ...validSummary(), totalTopics: "1" },
    });
    expect(store.status).toBe("completed_with_warnings");
  });

  it("lazily mounts the history browser", async () => {
    const store = useSyncSessionStore();
    store.open();
    const wrapper = mount(SyncSessionView);
    expect(wrapper.findComponent(SyncLogBrowserCore).exists()).toBe(false);
    await wrapper.get("#sync-history-tab").trigger("click");
    await vi.waitFor(() =>
      expect(wrapper.findComponent(SyncLogBrowserCore).exists()).toBe(true),
    );
    expect(wrapper.find("#sync-live-panel").exists()).toBe(false);
    await wrapper.get("#sync-live-tab").trigger("click");
    expect(wrapper.findComponent(SyncLogBrowserCore).exists()).toBe(false);
  });

  it("shows a recoverable history error without exposing backend details", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    mockInvoke("list_sync_log_files", () => {
      throw new Error("private-backend-detail");
    });
    const store = useSyncSessionStore();
    store.open();
    const wrapper = mount(SyncSessionView);
    try {
      await wrapper.get("#sync-history-tab").trigger("click");
      await vi.waitFor(() =>
        expect(wrapper.text()).toContain("无法加载同步日志，请稍后再试。"),
      );
      expect(wrapper.text()).toContain("重新加载");
      expect(wrapper.text()).not.toContain("private-backend-detail");
    } finally {
      wrapper.unmount();
      consoleError.mockRestore();
    }
  });
});
