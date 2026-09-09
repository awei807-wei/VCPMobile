// @vitest-environment happy-dom

import { afterEach, describe, expect, it } from "vitest";
import { mount, type VueWrapper } from "@vue/test-utils";
import RefreshButton from "../../../components/ui/RefreshButton.vue";

const wrappers: VueWrapper[] = [];

afterEach(() => {
  wrappers.splice(0).forEach((wrapper) => wrapper.unmount());
});

function mountRefreshButton(props: InstanceType<typeof RefreshButton>["$props"] = { label: "刷新" }) {
  const wrapper = mount(RefreshButton, { props });
  wrappers.push(wrapper);
  return wrapper;
}

function iconOf(wrapper: VueWrapper) {
  return wrapper.get("svg");
}

describe("RefreshButton", () => {
  it("后台 loading 从 false 变 true 时不自行旋转或刷新", async () => {
    const wrapper = mountRefreshButton();

    await wrapper.setProps({ loading: true });

    expect(iconOf(wrapper).classes()).not.toContain("ub-refresh-spinning");
    expect(wrapper.emitted("refresh")).toBeUndefined();
  });

  it("点击后 loading 变 true 且转过一圈时继续旋转并只刷新一次", async () => {
    const wrapper = mountRefreshButton();
    await wrapper.get("button").trigger("click");
    await wrapper.setProps({ loading: true });
    await iconOf(wrapper).trigger("animationiteration");

    expect(wrapper.emitted("refresh")).toHaveLength(1);
    expect(iconOf(wrapper).classes()).toContain("ub-refresh-spinning");
  });

  it("loading 变 false 后在下一次圈界事件停止旋转", async () => {
    const wrapper = mountRefreshButton({ label: "刷新", loading: true });
    await wrapper.get("button").trigger("click");
    expect(iconOf(wrapper).classes()).toContain("ub-refresh-spinning");

    await wrapper.setProps({ loading: false });
    await iconOf(wrapper).trigger("animationiteration");

    expect(iconOf(wrapper).classes()).not.toContain("ub-refresh-spinning");
  });

  it("disabled 后点击不发出 refresh", async () => {
    const wrapper = mountRefreshButton({ label: "刷新", disabled: true });

    await wrapper.get("button").trigger("click");

    expect(wrapper.emitted("refresh")).toBeUndefined();
    expect(iconOf(wrapper).classes()).not.toContain("ub-refresh-spinning");
  });
});
