import { defineComponent, h } from "vue";
import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import DistributedPluginsPanel from "@/features/distributed/components/DistributedPluginsPanel.vue";
import { useDistributedAuthorization } from "@/features/distributed/composables/useDistributedAuthorization";
import { useDistributedTools } from "@/features/distributed/composables/useDistributedTools";
import { invokeMock, mockInvoke } from "@/tests/mocks/tauri";

const streamingTool = {
  name: "MobileCPU",
  display_name: "移动 CPU",
  description: "读取 CPU 遥测",
  category: "streaming",
  placeholder: "{{MobileCPU}}",
  enabled: true,
};

function findButton(wrapper: ReturnType<typeof mount>, text: string) {
  return wrapper
    .findAll("button")
    .find((button) => button.text().includes(text));
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("分布式面板授权交互", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("get_registered_tools_metadata", () => [streamingTool]);
    mockInvoke("get_distributed_tool_config_status", () => ({
      enabled_names: ["MobileCPU"],
    }));
    mockInvoke("update_enabled_tools", () => undefined);
    mockInvoke("execute_distributed_tool", () => "温度: 40°C");
  });

  it("展开插件只浏览，必须点击显式按钮才读取 streaming", async () => {
    const wrapper = mount(DistributedPluginsPanel);
    await flushPromises();
    const header = wrapper.findAll("div.cursor-pointer")[0];
    await header?.trigger("click");
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "execute_distributed_tool",
      expect.anything(),
    );

    const readButton = findButton(wrapper, "读取实时数据");
    expect(readButton).toBeDefined();
    await readButton?.trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith("execute_distributed_tool", {
      name: "MobileCPU",
    });
  });

  it("授权持久化失败时保持原开关状态", async () => {
    const wrapper = mount(DistributedPluginsPanel);
    await flushPromises();
    mockInvoke("update_enabled_tools", () =>
      Promise.reject(new Error("写入失败")),
    );
    const toggle = wrapper.find('button[aria-label="移动 CPU授权开关"]');
    expect(toggle.attributes("class")).toContain("justify-end");
    await toggle.trigger("click");
    await flushPromises();
    expect(toggle.attributes("class")).toContain("justify-end");
  });

  it("快速双击授权开关只保留一个进行中的写入意图", async () => {
    const update = deferred<void>();
    mockInvoke("update_enabled_tools", () => update.promise);
    const wrapper = mount(DistributedPluginsPanel);
    await flushPromises();
    const toggle = wrapper.find('button[aria-label="移动 CPU授权开关"]');

    const firstClick = toggle.trigger("click");
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("update_enabled_tools", {
        enabledNames: [],
      }),
    );
    const secondClick = toggle.trigger("click");
    await Promise.all([firstClick, secondClick]);
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "update_enabled_tools",
      ),
    ).toHaveLength(1);
    expect(toggle.attributes("disabled")).toBeDefined();

    update.resolve();
    await flushPromises();
    expect(toggle.attributes("disabled")).toBeUndefined();
    expect(toggle.attributes("class")).toContain("justify-start");
  });

  it("授权状态读取失败时 fail-closed，不显示可用工具", async () => {
    mockInvoke("get_distributed_tool_config_status", () => {
      throw new Error("配置不可读");
    });
    const wrapper = mount(DistributedPluginsPanel);
    await flushPromises();
    expect(wrapper.text()).toContain("暂无匹配工具");
  });

  it("授权变更期间返回的旧元数据不会覆盖新状态", async () => {
    const metadata = deferred<unknown>();
    const config = deferred<unknown>();
    mockInvoke("get_registered_tools_metadata", () => metadata.promise);
    mockInvoke("get_distributed_tool_config_status", () => config.promise);
    mockInvoke("update_enabled_tools", () => undefined);

    let tools!: ReturnType<typeof useDistributedTools>;
    const wrapper = mount(
      defineComponent({
        setup() {
          tools = useDistributedTools();
          tools.clear();
          return () => h("div");
        },
      }),
    );
    const loading = tools.loadPluginsMetadata();
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("get_registered_tools_metadata"),
    );
    const authorization = useDistributedAuthorization();
    await authorization.updateEnabledTools(["MobileCPU"]);
    metadata.resolve([streamingTool]);
    config.resolve({ enabled_names: ["MobileCPU"] });
    await loading;
    expect(tools.pluginsList.value).toHaveLength(1);
    expect(tools.pluginsList.value[0].enabled).toBe(true);
    wrapper.unmount();
  });

  it("敏感工具未获系统权限时拒绝授权写入", async () => {
    mockInvoke("get_registered_tools_metadata", () => [
      {
        ...streamingTool,
        name: "MobileLocation",
        display_name: "移动定位",
      },
    ]);
    mockInvoke("get_distributed_tool_config_status", () => ({
      enabled_names: [],
    }));
    mockInvoke("plugin:vcp-mobile|check_all_permissions", () => ({
      location: false,
    }));
    mockInvoke("plugin:vcp-mobile|request_android_permission", () => undefined);
    const wrapper = mount(DistributedPluginsPanel);
    await flushPromises();
    const toggle = wrapper.find('button[aria-label="移动定位授权开关"]');
    await toggle.trigger("click");
    await flushPromises();
    expect(invokeMock).not.toHaveBeenCalledWith(
      "update_enabled_tools",
      expect.anything(),
    );
  });

  it("工具详情只接受同一工具的最新请求结果", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    let requestCount = 0;
    mockInvoke("execute_distributed_tool", () => {
      requestCount += 1;
      return requestCount === 1 ? first.promise : second.promise;
    });
    const tools = useDistributedTools();
    tools.clear();
    const plugin = {
      ...streamingTool,
      id: "MobileCPU",
      englishName: "MobileCPU",
      name: "移动 CPU",
      type: "streaming" as const,
      invocationCommands: [],
      icon: "",
      communication: undefined,
      requiresRoot: false,
      placeholder: undefined,
    };

    const oldRequest = tools.loadPluginDetails(plugin);
    await vi.waitFor(() => expect(requestCount).toBe(1));
    const latestRequest = tools.loadPluginDetails(plugin);
    second.resolve("新快照");
    await latestRequest;
    first.resolve("旧快照");
    await oldRequest;

    expect(tools.pluginData.value.MobileCPU).toBe("新快照");
    expect(tools.pluginLoading.value.MobileCPU).toBe(false);
  });

  it("清空详情状态后旧请求不能通过 ABA 序号覆盖新结果", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    let requestCount = 0;
    mockInvoke("execute_distributed_tool", () => {
      requestCount += 1;
      return requestCount === 1 ? first.promise : second.promise;
    });
    const tools = useDistributedTools();
    tools.clear();
    const plugin = {
      ...streamingTool,
      id: "MobileCPU",
      englishName: "MobileCPU",
      name: "移动 CPU",
      type: "streaming" as const,
      invocationCommands: [],
      icon: "",
      communication: undefined,
      requiresRoot: false,
      placeholder: undefined,
    };

    const oldRequest = tools.loadPluginDetails(plugin);
    await vi.waitFor(() => expect(requestCount).toBe(1));
    tools.clear();
    const latestRequest = tools.loadPluginDetails(plugin);
    second.resolve("清空后的新快照");
    await latestRequest;
    first.resolve("清空前的旧快照");
    await oldRequest;

    expect(tools.pluginData.value.MobileCPU).toBe("清空后的新快照");
  });
});
