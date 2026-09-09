// @vitest-environment happy-dom

import { createPinia, disposePinia, setActivePinia, type Pinia } from "pinia";
import { reactive } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useLogCenterStore } from "../../../features/logcenter/logCenterStore";
import {
  clearInvokeMocks,
  mockInvoke,
} from "../../mocks/tauri";

type TestSettings = {
  activeConnectionProfileId: string;
  vcpServerUrl: string;
  adminUsername: string;
  adminPassword: string;
  currentThemeMode?: string;
  agentOrder?: string[];
  [key: string]: unknown;
};

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
};

type FetchResult = {
  content: string;
  offset: number;
  path: string;
  fileSize: number;
  needFullReload: boolean;
};

type FetchCall = {
  args: Record<string, unknown>;
  deferred: Deferred<unknown>;
};

const mocks = vi.hoisted(() => ({
  settingsStore: null as { settings: TestSettings | null } | null,
  lifecycleStore: null as { isBackground: boolean } | null,
  switchGuardStore: null as { switching: boolean } | null,
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

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });
  return { promise, resolve, reject };
}

function settingsFor(id: string, url = `https://${id}.example.test`): TestSettings {
  return {
    activeConnectionProfileId: id,
    vcpServerUrl: url,
    adminUsername: `${id}-admin`,
    adminPassword: `${id}-secret`,
    currentThemeMode: "light",
    agentOrder: ["agent-a"],
  };
}

function result(
  content: string,
  offset: number,
  path = "/var/log/vcp.log",
  needFullReload = false,
): FetchResult {
  return {
    content,
    offset,
    path,
    fileSize: content.length,
    needFullReload,
  };
}

let pinia: Pinia | null = null;
let store: ReturnType<typeof useLogCenterStore> | null = null;
let requests: FetchCall[] = [];

function installFetchMock(): void {
  mockInvoke("logcenter_fetch", (args) => {
    const request = {
      args: { ...(args ?? {}) },
      deferred: deferred<unknown>(),
    };
    requests.push(request);
    return request.deferred.promise;
  });
}

function getStore(): ReturnType<typeof useLogCenterStore> {
  if (!store) store = useLogCenterStore();
  return store;
}

function getRequest(index: number): FetchCall {
  const request = requests[index];
  if (!request) throw new Error(`missing logcenter request ${index}`);
  return request;
}

async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  vi.runAllTicks();
  await Promise.resolve();
  await Promise.resolve();
}

async function advance(ms: number): Promise<void> {
  await vi.advanceTimersByTimeAsync(ms);
  await flushMicrotasks();
}

async function resolveRequest(index: number, value: unknown): Promise<void> {
  getRequest(index).deferred.resolve(value);
  await flushMicrotasks();
}

async function rejectRequest(index: number, error: unknown): Promise<void> {
  getRequest(index).deferred.reject(error);
  await flushMicrotasks();
}

async function openSession(): Promise<ReturnType<typeof useLogCenterStore>> {
  const current = getStore();
  current.startSession();
  current.pollOnce();
  await flushMicrotasks();
  return current;
}

beforeEach(() => {
  vi.useFakeTimers();
  clearInvokeMocks();
  pinia = createPinia();
  setActivePinia(pinia);
  mocks.settingsStore = reactive({ settings: settingsFor("lan") });
  mocks.lifecycleStore = reactive({ isBackground: false });
  mocks.switchGuardStore = reactive({ switching: false });
  requests = [];
  store = null;
  installFetchMock();
});

afterEach(() => {
  store?.$dispose();
  store = null;
  if (pinia) disposePinia(pinia);
  pinia = null;
  mocks.settingsStore = null;
  mocks.lifecycleStore = null;
  mocks.switchGuardStore = null;
  window.localStorage?.clear();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("logCenterStore T04 竞态与边界", () => {
  it("L01 未打开日志页推进 30 秒时不调用 invoke", async () => {
    getStore();

    await advance(30_000);

    expect(requests).toHaveLength(0);
  });

  it("L02 打开后首轮全量，下一次轮询使用增量和 offset=100", async () => {
    const current = await openSession();
    expect(getRequest(0).args).toMatchObject({ incremental: false, offset: 0 });

    await resolveRequest(0, result("首轮\n", 100));
    await advance(3_000);

    expect(getRequest(1).args).toMatchObject({ incremental: true, offset: 100 });
    expect(current.offset).toBe(100);
  });

  it("L03 A 未完成切 B 后 B 先成功，A 晚成功不能污染内容和 offset", async () => {
    const current = await openSession();
    mocks.settingsStore!.settings = settingsFor("wan", "https://b.example.test");
    await flushMicrotasks();

    expect(requests).toHaveLength(2);
    expect(getRequest(0).args.expectedServerUrl).toBe("https://lan.example.test");
    expect(getRequest(1).args).toMatchObject({
      incremental: false,
      offset: 0,
      expectedServerUrl: "https://b.example.test",
    });

    await resolveRequest(1, result("B 内容\n", 20, "/b.log"));
    await resolveRequest(0, result("A 内容\n", 10, "/a.log"));

    expect(current.lines).toEqual(["B 内容"]);
    expect(current.offset).toBe(20);
    expect(current.logPath).toBe("/b.log");
  });

  it("L04 A 未完成切 B 后 A 晚失败不能覆盖 B 的 error", async () => {
    const current = await openSession();
    mocks.settingsStore!.settings = settingsFor("wan", "https://b.example.test");
    await flushMicrotasks();

    await resolveRequest(1, result("B 内容\n", 20, "/b.log"));
    await rejectRequest(0, new Error("A 晚到错误"));

    expect(current.lines).toEqual(["B 内容"]);
    expect(current.offset).toBe(20);
    expect(current.error).toBeNull();
  });

  it("L05 同 id 修改服务器 URL 后清空并从新 URL 全量 offset=0", async () => {
    const current = await openSession();
    await resolveRequest(0, result("旧内容\n", 100));

    mocks.settingsStore!.settings = settingsFor("lan", "https://new.example.test");
    expect(current.lines).toEqual([]);
    expect(current.offset).toBe(0);
    await flushMicrotasks();

    expect(getRequest(1).args).toMatchObject({
      incremental: false,
      offset: 0,
      expectedServerUrl: "https://new.example.test",
    });
  });

  it.each([
    ["用户名", { adminUsername: "new-admin" }],
    ["密码", { adminPassword: "new-secret" }],
  ])("L06 同 id 修改%s使旧结果失效且 invoke 不携带凭据", async (_field, update) => {
    const current = await openSession();
    mocks.settingsStore!.settings = {
      ...settingsFor("lan"),
      ...update,
    };
    await flushMicrotasks();

    const args = getRequest(1).args;
    expect(args).toMatchObject({ incremental: false, offset: 0 });
    expect(Object.prototype.hasOwnProperty.call(args, "adminUsername")).toBe(false);
    expect(Object.prototype.hasOwnProperty.call(args, "adminPassword")).toBe(false);

    await resolveRequest(1, result("新内容\n", 200));
    await resolveRequest(0, result("旧内容\n", 100));
    expect(current.lines).toEqual(["新内容"]);
    expect(current.offset).toBe(200);
  });

  it.each([
    ["关闭页面", (current: ReturnType<typeof useLogCenterStore>) => current.stopSession()],
    ["退后台", () => { mocks.lifecycleStore!.isBackground = true; }],
    ["暂停", (current: ReturnType<typeof useLogCenterStore>) => current.togglePause()],
  ])("L07 %s 后当前响应完成不写入且不产生下一轮", async (_name, transition) => {
    const current = await openSession();
    transition(current);
    await flushMicrotasks();
    await resolveRequest(0, result("不应显示\n", 100));
    await advance(30_000);

    expect(current.lines).toEqual([]);
    expect(current.offset).toBe(0);
    expect(requests).toHaveLength(1);
  });

  it("L08 暂停和后台恢复后的第一轮都使用全量 offset=0", async () => {
    const current = await openSession();
    await resolveRequest(0, result("初始\n", 100));

    mocks.lifecycleStore!.isBackground = true;
    await flushMicrotasks();
    mocks.lifecycleStore!.isBackground = false;
    await flushMicrotasks();
    expect(getRequest(1).args).toMatchObject({ incremental: false, offset: 0 });
    await resolveRequest(1, result("前台恢复\n", 200));

    current.togglePause();
    await flushMicrotasks();
    current.togglePause();
    await flushMicrotasks();
    expect(getRequest(2).args).toMatchObject({ incremental: false, offset: 0 });
  });

  it("L09 连续刷新 20 次且前两个 deferred 未完成时在途 invoke 不超过 2", async () => {
    const current = await openSession();
    current.refresh();
    await flushMicrotasks();
    for (let index = 0; index < 20; index += 1) current.refresh();
    await flushMicrotasks();

    expect(requests).toHaveLength(2);
    expect(requests.filter((request) => request.deferred.promise).length).toBe(2);
  });

  it("L10 L09 中旧请求完成只启动最新待执行代际，不补发 20 个旧任务", async () => {
    const current = await openSession();
    current.refresh();
    await flushMicrotasks();
    for (let index = 0; index < 20; index += 1) current.refresh();
    await flushMicrotasks();

    await resolveRequest(0, result("旧 A\n", 10));
    expect(requests).toHaveLength(3);
    expect(getRequest(2).args).toMatchObject({ incremental: false, offset: 0 });

    await resolveRequest(2, result("最新\n", 30));
    await resolveRequest(1, result("旧 B\n", 20));
    expect(current.lines).toEqual(["最新"]);
    expect(requests).toHaveLength(3);
  });

  it("L11 新请求进行中时旧请求 finally 执行仍保持 isLoading=true", async () => {
    const current = await openSession();
    current.refresh();
    await flushMicrotasks();

    await resolveRequest(0, result("旧\n", 10));
    expect(current.isLoading).toBe(true);
    expect(requests).toHaveLength(2);
  });

  it("L12 增量半行与下一次行尾拼接为一个不重复的半行", async () => {
    const current = await openSession();
    await resolveRequest(0, result("半", 1));
    await advance(3_000);
    expect(getRequest(1).args).toMatchObject({ incremental: true, offset: 1 });

    await resolveRequest(1, result("行\n", 3));
    expect(current.lines).toEqual(["半行"]);
    expect(current.pendingFragment).toBe("");
  });

  it("L13 增量 content 为空时不增加新行徽标或额外日志行", async () => {
    const current = await openSession();
    await resolveRequest(0, result("已有\n", 100));
    await advance(3_000);
    const beforeLines = [...current.lines];
    const beforeVersion = current.logVersion;

    await resolveRequest(1, result("", 100));
    expect(current.lines).toEqual(beforeLines);
    expect(current.newLineCount).toBe(0);
    expect(current.logVersion).toBe(beforeVersion);
  });

  it("L14 增量 needFullReload 只触发一次 false/0 全量并清空旧缓冲", async () => {
    const current = await openSession();
    await resolveRequest(0, result("旧快照\n半", 100));
    await advance(3_000);

    await resolveRequest(1, result("", 0, "/rotated.log", true));
    expect(current.lines).toEqual([]);
    expect(current.pendingFragment).toBe("");
    expect(getRequest(2).args).toMatchObject({ incremental: false, offset: 0 });

    await resolveRequest(2, result("新快照\n", 50, "/rotated.log"));
    expect(current.lines).toEqual(["新快照"]);
  });

  it("L15 全量连续 needFullReload 按 6s→12s→24s→30s→30s 退避", async () => {
    const current = await openSession();
    await resolveRequest(0, result("", 0, "/rotated.log", true));
    expect(current.error).toContain("全量响应仍要求重载");
    expect(requests).toHaveLength(1);

    await advance(5_999);
    expect(requests).toHaveLength(1);
    await advance(1);
    expect(requests).toHaveLength(2);
    await resolveRequest(1, result("", 0, "/rotated.log", true));
    expect(requests).toHaveLength(2);
    await advance(11_999);
    expect(requests).toHaveLength(2);
    await advance(1);
    expect(requests).toHaveLength(3);
    await resolveRequest(2, result("", 0, "/rotated.log", true));
    await advance(23_999);
    expect(requests).toHaveLength(3);
    await advance(1);
    expect(requests).toHaveLength(4);
    await resolveRequest(3, result("", 0, "/rotated.log", true));
    await advance(29_999);
    expect(requests).toHaveLength(4);
    await advance(1);
    expect(requests).toHaveLength(5);
    await resolveRequest(4, result("", 0, "/rotated.log", true));
    await advance(29_999);
    expect(requests).toHaveLength(5);
    await advance(1);
    expect(requests).toHaveLength(6);
    expect(current.isLoading).toBe(true);
  });

  it.each([
    ["负 offset", { content: "坏数据\n", offset: -1, path: "/bad", fileSize: 4, needFullReload: false }],
    ["非安全整数 offset", { content: "坏数据\n", offset: Number.MAX_SAFE_INTEGER + 1, path: "/bad", fileSize: 4, needFullReload: false }],
    ["缺失字段", { content: "坏数据\n", offset: 101, path: "/bad", fileSize: 4 }],
  ])("L16 %s 响应只产生可见错误且不污染已验证缓冲", async (_name, invalid) => {
    const current = await openSession();
    await resolveRequest(0, result("已验证\n", 100));
    await advance(3_000);
    await resolveRequest(1, invalid);

    expect(current.error).toBeTruthy();
    expect(current.lines).toEqual(["已验证"]);
    expect(current.offset).toBe(100);
  });

  it("L17 超长行和大量行连续追加受行数、单行和总字符数限制", async () => {
    const current = await openSession();
    const longLine = "z".repeat(70_000);
    const content = `${Array.from({ length: 700 }, () => "x".repeat(3_000)).join("\n")}\n${longLine}\n`;
    await resolveRequest(0, result(content, content.length));

    const totalCharacters = current.lines.reduce((sum, line) => sum + line.length, 0);
    const retainedLongLine = current.lines[current.lines.length - 1];
    expect(retainedLongLine.startsWith("[前部日志已省略] ")).toBe(true);
    expect(retainedLongLine).toHaveLength(65_536);
    expect(current.lines.length).toBeGreaterThan(300);
    expect(current.lines.length).toBeLessThanOrEqual(current.lineLimit);
    expect(Math.max(...current.lines.map((line) => line.length))).toBeLessThanOrEqual(65_536);
    expect(totalCharacters).toBeLessThanOrEqual(1_048_576);
  });

  it("L18 setLineLimit(100) 只保留最后 100 行且不改变服务器 offset", async () => {
    const current = await openSession();
    const content = `${Array.from({ length: 500 }, (_, index) => `line-${index}`).join("\n")}\n`;
    await resolveRequest(0, result(content, 500));

    current.setLineLimit(100);
    expect(current.lineLimit).toBe(100);
    expect(current.lines).toHaveLength(100);
    expect(current.lines[0]).toBe("line-400");
    expect(current.lines[99]).toBe("line-499");
    expect(current.offset).toBe(500);
  });

  it("L19 $dispose() 后旧 promise 完成不新建 timer 或 invoke", async () => {
    const current = await openSession();
    current.$dispose();
    await resolveRequest(0, result("已销毁\n", 100));
    await advance(30_000);

    expect(current.lines).toEqual([]);
    expect(requests).toHaveLength(1);
  });

  it("L20 线路切换失败返回原配置时清空旧响应并重新全量", async () => {
    const current = await openSession();
    mocks.switchGuardStore!.switching = true;
    await flushMicrotasks();
    await resolveRequest(0, result("切换前\n", 100));
    expect(current.lines).toEqual([]);

    mocks.switchGuardStore!.switching = false;
    await flushMicrotasks();
    expect(getRequest(1).args).toMatchObject({
      incremental: false,
      offset: 0,
      expectedServerUrl: "https://lan.example.test",
    });
    await resolveRequest(1, result("原配置恢复\n", 120));
    expect(current.lines).toEqual(["原配置恢复"]);
  });

  it("L21 修改主题和排序等无关设置不清空日志或新建请求代际", async () => {
    const current = await openSession();
    await resolveRequest(0, result("保留\n", 100));
    const beforeLines = [...current.lines];
    const beforeRequests = requests.length;

    mocks.settingsStore!.settings = {
      ...mocks.settingsStore!.settings!,
      currentThemeMode: "dark",
      agentOrder: ["agent-b"],
    };
    await flushMicrotasks();

    expect(current.lines).toEqual(beforeLines);
    expect(current.offset).toBe(100);
    expect(requests).toHaveLength(beforeRequests);
  });
});
