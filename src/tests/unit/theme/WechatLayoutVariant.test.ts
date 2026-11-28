// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useThemeStore } from "../../../core/stores/theme";

const customThemeTag = () => document.getElementById("vcp-custom-theme");

describe("微信布局变体", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    localStorage.clear();
    document.documentElement.removeAttribute("data-vcp-layout");
    customThemeTag()?.remove();
  });

  it("切到微信主题写入 wechat，切回普通主题回退 default 且不残留样式", async () => {
    const store = useThemeStore();

    await store.applyThemeFile("themes-wechat.css");
    expect(store.layoutVariant).toBe("wechat");
    expect(store.isWechatLayout).toBe(true);
    expect(document.documentElement.dataset.vcpLayout).toBe("wechat");
    expect(customThemeTag()?.textContent ?? "").toContain("vcp-wechat-tail");

    await store.applyThemeFile("themes-simple-bw.css");
    expect(store.layoutVariant).toBe("default");
    expect(store.isWechatLayout).toBe(false);
    expect(document.documentElement.dataset.vcpLayout).toBe("default");
    expect(customThemeTag()?.textContent ?? "").not.toContain(
      "vcp-wechat-tail",
    );
  });

  it("仅用户主动选中主题时才应用其默认明暗模式", async () => {
    localStorage.setItem("vcp-theme-mode", "dark");
    const store = useThemeStore();
    expect(store.mode).toBe("dark");

    // 冷启动 initTheme / 后端推送都不带标记：保留用户手动选择的暗色
    await store.applyThemeFile("themes-wechat.css");
    expect(store.mode).toBe("dark");

    // 用户主动在主题选择器中选中：切到主题声明的浅色
    await store.applyThemeFile("themes-wechat.css", { fromUserPicker: true });
    expect(store.mode).toBe("light");
  });
});
