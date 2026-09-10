// @vitest-environment happy-dom

import { createPinia, disposePinia, setActivePinia, type Pinia } from "pinia";
import { nextTick, reactive } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import FeatureOverlays from "../../../components/FeatureOverlays.vue";
import RightSidebar from "../../../components/layout/RightSidebar.vue";
import BottomSheet from "../../../components/ui/BottomSheet.vue";
import RefreshButton from "../../../components/ui/RefreshButton.vue";
import { useModalHistory } from "../../../core/composables/useModalHistory";
import { useOverlayStore } from "../../../core/stores/overlay";
import LogCenterView from "../../../features/logcenter/LogCenterView.vue";
import invokeGuardSource from "../../../../src-tauri/src/vcp_modules/infra/invoke_guard.rs?raw";
import tauriLibSource from "../../../../src-tauri/src/lib.rs?raw";

type LogStoreMock = {
  lines: string[];
  displayedLines: string[];
  pendingFragment: string;
  offset: number;
  logPath: string;
  fileSize: number;
  isPaused: boolean;
  isLoading: boolean;
  error: string | null;
  lineLimit: number;
  isReverse: boolean;
  autoScroll: boolean;
  filterText: string;
  newLineCount: number;
  logVersion: number;
  totalBuffered: number;
  matchedCount: number;
  isPolling: boolean;
  requestBlocked: boolean;
  sourceLabel: string;
  startSession: ReturnType<typeof vi.fn>;
  stopSession: ReturnType<typeof vi.fn>;
  resetSession: ReturnType<typeof vi.fn>;
  refresh: ReturnType<typeof vi.fn>;
  togglePause: ReturnType<typeof vi.fn>;
  setLineLimit: ReturnType<typeof vi.fn>;
  toggleReverse: ReturnType<typeof vi.fn>;
  toggleAutoScroll: ReturnType<typeof vi.fn>;
  acknowledgeNewLines: ReturnType<typeof vi.fn>;
  pollOnce: ReturnType<typeof vi.fn>;
};

type NotificationStoreMock = {
  isDrawerOpen: boolean;
  historyList: unknown[];
  unreadCount: number;
  vcpStatus: { status: string; message: string; source: string };
  addNotification: ReturnType<typeof vi.fn>;
  markAllRead: ReturnType<typeof vi.fn>;
  clearHistory: ReturnType<typeof vi.fn>;
  removeHistoryItem: ReturnType<typeof vi.fn>;
  updateStatus: ReturnType<typeof vi.fn>;
  updateCoreStatus: ReturnType<typeof vi.fn>;
};

type SettingsStoreMock = {
  settings: {
    activeConnectionProfileId: string;
    vcpServerUrl: string;
    adminUsername: string;
    adminPassword: string;
    distributedEnabled: boolean;
  } | null;
};

const mocks = vi.hoisted(() => ({
  logStore: null as LogStoreMock | null,
  notificationStore: null as NotificationStoreMock | null,
  settingsStore: null as SettingsStoreMock | null,
  lifecycleStore: null as { isBackground: boolean } | null,
  switchGuardStore: null as { switching: boolean } | null,
  chatStreamStore: null as { hasActiveStreams: boolean } | null,
  connectionProfilesStore: null as {
    hasFloatingAssistantGenerating: boolean;
    hasModelRefreshInFlight: boolean;
    switching: boolean;
    hasActiveSyncSession: boolean;
    targetProfileName: string;
    activeProfileName: string;
    canSwitch: boolean;
    switchToTarget: ReturnType<typeof vi.fn>;
  } | null,
  invoke: vi.fn(),
}));

vi.mock("../../../features/logcenter/logCenterStore", () => ({
  useLogCenterStore: () => mocks.logStore,
}));

vi.mock("../../../core/stores/notification", () => ({
  useNotificationStore: () => mocks.notificationStore,
}));

vi.mock("../../../core/stores/settings", () => ({
  useSettingsStore: () => mocks.settingsStore,
}));

vi.mock("../../../core/stores/appLifecycle", () => ({
  useAppLifecycleStore: () => mocks.lifecycleStore,
}));

vi.mock("../../../core/stores/connectionSwitchGuard", () => ({
  useConnectionSwitchGuardStore: () => mocks.switchGuardStore,
}));

vi.mock("../../../core/stores/chatStreamStore", () => ({
  useChatStreamStore: () => mocks.chatStreamStore,
}));

vi.mock("../../../core/stores/connectionProfiles", () => ({
  useConnectionProfilesStore: () => mocks.connectionProfilesStore,
}));

vi.mock("../../../core/composables/useSidebarSwipe", () => ({
  useSidebarSwipe: vi.fn(() => ({})),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => mocks.invoke(...args),
}));

// FeatureOverlays 里的其他异步页不是本集成测试的对象；替换其模块，避免测试结束后
// 仍在解析与本场景无关的懒加载依赖。LogCenterView 保持真实组件以验证日志页接线。
vi.mock("../../../features/agent/AgentSettingsView.vue", () => ({
  default: { template: "<div />" },
}));
vi.mock("../../../features/agent/GroupSettingsView.vue", () => ({
  default: { template: "<div />" },
}));
vi.mock("../../../features/sync/SyncSessionView.vue", () => ({
  default: { template: "<div />" },
}));
vi.mock("../../../features/settings/components/RebuildSessionView.vue", () => ({
  default: { template: "<div />" },
}));
vi.mock("../../../features/distributed/DistributedView.vue", () => ({
  default: { template: "<div />" },
}));
vi.mock("../../../features/settings/SettingsView.vue", () => ({
  default: { template: "<div />" },
}));
vi.mock("../../../features/dailynote/DailyNoteView.vue", () => ({
  default: { template: "<div />" },
}));
vi.mock("../../../features/rag/RagObserver.vue", () => ({
  default: { template: "<div />" },
}));
vi.mock("../../../features/globalsearch/GlobalSearchView.vue", () => ({
  default: { template: "<div />" },
}));

function createLogStore(): LogStoreMock {
  const store = reactive({
    lines: [],
    displayedLines: [],
    pendingFragment: "",
    offset: 0,
    logPath: "",
    fileSize: 0,
    isPaused: false,
    isLoading: false,
    error: null,
    lineLimit: 500,
    isReverse: false,
    autoScroll: true,
    filterText: "",
    newLineCount: 0,
    logVersion: 0,
    totalBuffered: 0,
    matchedCount: 0,
    isPolling: true,
    requestBlocked: false,
    sourceLabel: "内网",
    startSession: vi.fn(),
    stopSession: vi.fn(),
    resetSession: vi.fn(),
    refresh: vi.fn(),
    togglePause: vi.fn(),
    setLineLimit: vi.fn(),
    toggleReverse: vi.fn(),
    toggleAutoScroll: vi.fn(),
    acknowledgeNewLines: vi.fn(),
    pollOnce: vi.fn(),
  }) as LogStoreMock;

  store.startSession.mockImplementation(() => {
    store.isPolling = true;
  });
  store.stopSession.mockImplementation(() => {
    store.isPolling = false;
  });
  store.setLineLimit.mockImplementation((limit: number) => {
    store.lineLimit = limit;
    store.lines = store.lines.slice(-limit);
    store.displayedLines = store.displayedLines.slice(-limit);
    store.totalBuffered = store.lines.length;
    store.matchedCount = store.displayedLines.length;
  });

  return store;
}

function createNotificationStore(): NotificationStoreMock {
  return reactive({
    isDrawerOpen: false,
    historyList: [],
    unreadCount: 0,
    vcpStatus: { status: "connecting", message: "", source: "VCPLog" },
    addNotification: vi.fn(),
    markAllRead: vi.fn(),
    clearHistory: vi.fn(),
    removeHistoryItem: vi.fn(),
    updateStatus: vi.fn(),
    updateCoreStatus: vi.fn(),
  }) as NotificationStoreMock;
}

function createSettingsStore(): SettingsStoreMock {
  return reactive({
    settings: {
      activeConnectionProfileId: "lan",
      vcpServerUrl: "https://lan.example.test",
      adminUsername: "lan-admin",
      adminPassword: "lan-password",
      distributedEnabled: false,
    },
  });
}

const wrappers: VueWrapper[] = [];
let pinia: Pinia | null = null;

function clearModalHistory(): void {
  const history = useModalHistory();
  while (history.closeTopModal()) {
    // 页面回调会同步清理对应的 pageStack；这里仅清掉模块级历史栈。
  }
  window.history.replaceState({ vcpRoot: true, vcpMain: true }, "");
}

beforeEach(() => {
  mocks.logStore = createLogStore();
  mocks.notificationStore = createNotificationStore();
  mocks.settingsStore = createSettingsStore();
  mocks.lifecycleStore = reactive({ isBackground: false });
  mocks.switchGuardStore = reactive({ switching: false });
  mocks.chatStreamStore = reactive({ hasActiveStreams: false });
  mocks.connectionProfilesStore = {
    hasFloatingAssistantGenerating: false,
    hasModelRefreshInFlight: false,
    switching: false,
    hasActiveSyncSession: false,
    targetProfileName: "外网",
    activeProfileName: "内网",
    canSwitch: true,
    switchToTarget: vi.fn(),
  };
  mocks.invoke.mockReset();
  pinia = createPinia();
  setActivePinia(pinia);
  clearModalHistory();
});

afterEach(() => {
  wrappers.splice(0).forEach((wrapper) => wrapper.unmount());
  document.body.innerHTML = "";
  clearModalHistory();
  if (pinia) disposePinia(pinia);
  pinia = null;
  vi.useRealTimers();
  vi.clearAllMocks();
});

function mountLogCenter(props: { isOpen?: boolean; isActive?: boolean } = {}) {
  const wrapper = mount(LogCenterView, {
    props: {
      isOpen: true,
      isActive: true,
      ...props,
    },
  });
  wrappers.push(wrapper);
  return wrapper;
}

function mountFeatureOverlays() {
  const wrapper = mount(FeatureOverlays, {
    global: {
      stubs: {
        AgentSettingsView: true,
        GroupSettingsView: true,
        SettingsView: true,
        TarvenSettingsView: true,
        SyncSessionView: true,
        RebuildSessionView: true,
        DistributedView: true,
        DailyNoteView: true,
        RagObserverView: true,
        GlobalSearchView: true,
        ToolInteractionOverlay: true,
      },
    },
  });
  wrappers.push(wrapper);
  return wrapper;
}

function mountRightSidebar() {
  const wrapper = mount(RightSidebar, {
    props: { isOpen: true },
    global: {
      stubs: {
        NotificationStatusBar: true,
        NotificationList: true,
      },
    },
  });
  wrappers.push(wrapper);
  return wrapper;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });
  return { promise, resolve, reject };
}

function fetchResult(content: string, offset: number, path = "/var/log/vcp.log") {
  return {
    content,
    offset,
    path,
    fileSize: content.length,
    needFullReload: false,
  };
}

async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  await nextTick();
  await Promise.resolve();
  await Promise.resolve();
}

describe("LogCenterView T05 集成", () => {
  it("只有一个真实 BottomSheet，菜单无 compact 和 clear 操作", async () => {
    const wrapper = mountLogCenter();
    const sheets = wrapper.findAllComponents(BottomSheet);

    expect(sheets).toHaveLength(1);
    const sheetProps = sheets[0].props() as unknown as Record<string, unknown>;
    expect(sheetProps.compact).toBeUndefined();

    await wrapper.get('button[aria-label="更多操作"]').trigger("click");
    const actions = sheets[0].props("actions") as Array<{ label: string }>;
    expect(actions.map((action) => action.label)).toEqual([
      "关闭自动滚动",
      "复制可见日志",
    ]);
    expect(wrapper.text()).not.toContain("清空");
  });

  it("原生行数 select 变更调用 Store 的 setLineLimit", async () => {
    const wrapper = mountLogCenter();

    await wrapper.get('select[aria-label="日志行数上限"]').setValue("100");

    expect(mocks.logStore!.setLineLimit).toHaveBeenCalledWith(100);
  });

  it("刷新按钮挂载真实组件并调用适配版 Store", async () => {
    const wrapper = mountLogCenter();
    const refreshButton = wrapper.findComponent(RefreshButton);

    expect(refreshButton.exists()).toBe(true);
    expect(refreshButton.props("disabled")).toBe(false);
    await refreshButton.get("button").trigger("click");

    expect(mocks.logStore!.refresh).toHaveBeenCalledTimes(1);
  });

  it("非 active 页面不启动会话，后台 requestBlocked 时刷新按钮不可用", async () => {
    const inactive = mountLogCenter({ isActive: false });
    expect(mocks.logStore!.startSession).not.toHaveBeenCalled();

    await inactive.setProps({ isActive: true });
    expect(mocks.logStore!.startSession).toHaveBeenCalledTimes(1);
    await inactive.setProps({ isActive: false });
    expect(mocks.logStore!.stopSession).toHaveBeenCalled();

    mocks.logStore!.requestBlocked = true;
    const background = mountLogCenter();
    const refreshButton = background.findComponent(RefreshButton);
    expect(refreshButton.props("disabled")).toBe(true);
    await refreshButton.get("button").trigger("click");
    expect(mocks.logStore!.refresh).not.toHaveBeenCalled();
  });
});

describe("日志中心 T07 集成门禁", () => {
  it("右栏点击日志入口后打开日志页并关闭右栏，页面栈只有一个 logCenter", async () => {
    const sidebar = mountRightSidebar();
    const logEntry = sidebar
      .findAll("button")
      .find((button) => button.text().includes("日志中心 · 只读"));

    expect(logEntry).toBeDefined();
    await logEntry!.trigger("click");

    const overlay = useOverlayStore();
    expect(overlay.isLogCenterOpen).toBe(true);
    expect(overlay.isLogCenterActive).toBe(true);
    expect(overlay.pageStack.filter((page) => page.type === "logCenter")).toHaveLength(1);
    expect(sidebar.emitted("close")).toHaveLength(1);
  });

  it("快速重复点击日志入口不会重复压入 pageStack", async () => {
    const sidebar = mountRightSidebar();
    const logEntry = sidebar
      .findAll("button")
      .find((button) => button.text().includes("日志中心 · 只读"));

    expect(logEntry).toBeDefined();
    await logEntry!.trigger("click");
    await logEntry!.trigger("click");

    const overlay = useOverlayStore();
    expect(overlay.pageStack.filter((page) => page.type === "logCenter")).toHaveLength(1);
  });

  it("Android 返回先由 ModalHistory 关闭 BottomSheet，再关闭日志页且保留其他页面", async () => {
    const overlay = useOverlayStore();
    overlay.openSettings();
    overlay.openLogCenter();
    const wrapper = mountLogCenter();

    await wrapper.get('button[aria-label="更多操作"]').trigger("click");
    const sheet = wrapper.findComponent(BottomSheet);
    expect(sheet.props("modelValue")).toBe(true);

    window.dispatchEvent(
      new PopStateEvent("popstate", {
        state: { vcpRoot: true, vcpMain: true },
      }),
    );
    await nextTick();
    expect(sheet.props("modelValue")).toBe(false);
    expect(overlay.pageStack.map((page) => page.type)).toEqual(["settings", "logCenter"]);

    window.dispatchEvent(
      new PopStateEvent("popstate", {
        state: { vcpRoot: true, vcpMain: true },
      }),
    );
    await nextTick();
    expect(overlay.isLogCenterOpen).toBe(false);
    expect(overlay.pageStack.map((page) => page.type)).toEqual(["settings"]);
    expect(overlay.isSettingsOpen).toBe(true);
  });

  it("其他业务页覆盖日志时保持 isLogCenterOpen 但停止 active 会话轮询", () => {
    const overlay = useOverlayStore();
    overlay.openLogCenter();
    overlay.openSettings();

    expect(overlay.isLogCenterOpen).toBe(true);
    expect(overlay.isLogCenterActive).toBe(false);

    mountLogCenter({
      isOpen: overlay.isLogCenterOpen,
      isActive: overlay.isLogCenterActive,
    });

    expect(mocks.logStore!.startSession).not.toHaveBeenCalled();
    expect(mocks.logStore!.stopSession).toHaveBeenCalledTimes(1);
    expect(mocks.logStore!.isPolling).toBe(false);
  });

  it("从覆盖页返回日志页时重新全量拉取，后续轮询才使用增量", async () => {
    vi.useFakeTimers();
    const first = deferred<ReturnType<typeof fetchResult>>();
    const resumed = deferred<ReturnType<typeof fetchResult>>();
    const incremental = deferred<ReturnType<typeof fetchResult>>();
    const pending = [first, resumed, incremental];
    mocks.invoke.mockImplementation(() => pending.shift()!.promise);

    const actual = await vi.importActual<typeof import("../../../features/logcenter/logCenterStore")>(
      "../../../features/logcenter/logCenterStore",
    );
    const store = actual.useLogCenterStore();

    store.startSession();
    store.pollOnce();
    await flushMicrotasks();
    expect(mocks.invoke).toHaveBeenCalledTimes(1);
    expect(mocks.invoke.mock.calls[0]?.[1]).toMatchObject({
      incremental: false,
      offset: 0,
    });

    first.resolve(fetchResult("覆盖页首轮\n", 10, "/lan.log"));
    await flushMicrotasks();
    store.stopSession();
    expect(store.isPolling).toBe(false);

    store.startSession();
    store.pollOnce();
    await flushMicrotasks();
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
    expect(mocks.invoke.mock.calls[1]?.[1]).toMatchObject({
      incremental: false,
      offset: 0,
    });

    resumed.resolve(fetchResult("恢复页首轮\n", 20, "/lan.log"));
    await flushMicrotasks();
    await vi.advanceTimersByTimeAsync(3000);
    await flushMicrotasks();
    expect(mocks.invoke).toHaveBeenCalledTimes(3);
    expect(mocks.invoke.mock.calls[2]?.[1]).toMatchObject({
      incremental: true,
      offset: 20,
    });

    incremental.resolve(fetchResult("恢复页增量\n", 30, "/lan.log"));
    await flushMicrotasks();
    store.$dispose();
  });

  it("日志页首次异步加载期间立即关闭，组件晚加载后不发送请求", async () => {
    const overlay = useOverlayStore();
    const featureOverlays = mountFeatureOverlays();

    await nextTick();
    overlay.openLogCenter();
    await nextTick();
    overlay.closeLogCenter();
    await flushMicrotasks();

    expect(featureOverlays.exists()).toBe(true);
    expect(overlay.isLogCenterOpen).toBe(false);
    expect(mocks.logStore!.startSession).not.toHaveBeenCalled();
  });

  it("script、HTML 标签和 ANSI 日志都作为普通文本显示，不执行 HTML", async () => {
    const unsafeLines = [
      "<script>window.__logCenterExecuted = true</script>",
      "<b>HTML 日志</b>",
      "\u001b[31mANSI 日志\u001b[0m",
    ];
    mocks.logStore!.lines = unsafeLines;
    mocks.logStore!.displayedLines = unsafeLines;
    mocks.logStore!.totalBuffered = unsafeLines.length;
    mocks.logStore!.matchedCount = unsafeLines.length;

    const wrapper = mountLogCenter();
    await nextTick();

    expect(wrapper.text()).toContain(unsafeLines[0]);
    expect(wrapper.text()).toContain(unsafeLines[1]);
    expect(wrapper.text()).toContain(unsafeLines[2]);
    expect(wrapper.find("script").exists()).toBe(false);
    expect(wrapper.find("b").exists()).toBe(false);
    expect((globalThis as { __logCenterExecuted?: boolean }).__logCenterExecuted).not.toBe(true);
  });

  it("3000 行降到 100 行后虚拟窗口恢复并继续显示日志", async () => {
    const rows = Array.from({ length: 3000 }, (_, index) => `日志行-${index + 1}`);
    mocks.logStore!.lineLimit = 3000;
    mocks.logStore!.lines = rows;
    mocks.logStore!.displayedLines = rows;
    mocks.logStore!.totalBuffered = rows.length;
    mocks.logStore!.matchedCount = rows.length;

    const wrapper = mountLogCenter();
    await nextTick();
    const scroll = wrapper.get('[data-logcenter-role="log-scroll"]');
    Object.defineProperty(scroll.element, "clientHeight", {
      configurable: true,
      value: 220,
    });
    Object.defineProperty(scroll.element, "scrollHeight", {
      configurable: true,
      get: () => mocks.logStore!.displayedLines.length * 22,
    });
    scroll.element.scrollTop = 60_000;
    await scroll.trigger("scroll");

    await wrapper.get('select[aria-label="日志行数上限"]').setValue("100");
    await flushMicrotasks();
    await nextTick();
    await nextTick();

    expect(mocks.logStore!.setLineLimit).toHaveBeenCalledWith(100);
    expect(wrapper.find('[data-logcenter-role="log-scroll"]').exists()).toBe(true);
    expect(wrapper.findAll(".log-row").length).toBeGreaterThan(0);
    expect(wrapper.find(".log-empty").exists()).toBe(false);
  });

  it("切换线路时更新来源标签并清除旧日志数据", async () => {
    const store = mocks.logStore!;
    store.lines = ["内网旧日志"];
    store.displayedLines = ["内网旧日志"];
    store.totalBuffered = 1;
    store.matchedCount = 1;
    const wrapper = mountLogCenter();
    await nextTick();
    expect(wrapper.text()).toContain("内网");
    expect(wrapper.text()).toContain("内网旧日志");

    store.sourceLabel = "外网";
    store.lines = [];
    store.displayedLines = [];
    store.offset = 0;
    store.totalBuffered = 0;
    store.matchedCount = 0;
    await nextTick();
    expect(wrapper.text()).toContain("外网");
    expect(wrapper.text()).not.toContain("内网旧日志");

    store.lines = ["外网新日志"];
    store.displayedLines = ["外网新日志"];
    store.totalBuffered = 1;
    store.matchedCount = 1;
    await nextTick();
    expect(wrapper.text()).toContain("外网新日志");
    expect(wrapper.text()).not.toContain("内网旧日志");
  });

  it("数据库未就绪时日志命令仍受启动门禁拦截且未加入白名单", () => {
    const allowlistStart = invokeGuardSource.indexOf("pub fn can_run_before_db_ready");
    const allowlistEnd = invokeGuardSource.indexOf("/// 判断当前 IPC", allowlistStart);
    expect(allowlistStart).toBeGreaterThanOrEqual(0);
    expect(allowlistEnd).toBeGreaterThan(allowlistStart);

    const allowlist = invokeGuardSource.slice(allowlistStart, allowlistEnd);
    expect(allowlist).toContain('command.starts_with("plugin:")');
    expect(allowlist).not.toContain("logcenter_fetch");
    expect(tauriLibSource).toMatch(/\blogcenter_fetch\b/);
  });
});
