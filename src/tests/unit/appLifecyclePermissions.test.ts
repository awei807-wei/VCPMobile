import { describe, expect, it } from "vitest";
import {
  hasRequiredStartupPermissions,
  type PermissionStatus,
} from "@/core/stores/appLifecycle";

const permissionStatus = (
  overrides: Partial<PermissionStatus> = {},
): PermissionStatus => ({
  notification: true,
  ring: true,
  storage: true,
  battery: true,
  backgroundRestricted: false,
  requiresManualPowerManagement: false,
  microphone: false,
  camera: false,
  overlay: false,
  location: false,
  ...overrides,
});

describe("应用生命周期启动权限聚合", () => {
  it("必需权限齐全时允许可选能力不可用", () => {
    expect(
      hasRequiredStartupPermissions(permissionStatus(), { enabled: true }),
    ).toBe(true);
  });

  it.each(["microphone", "camera", "overlay", "location"] as const)(
    "可选权限 %s 不应阻塞启动",
    (key) => {
      expect(
        hasRequiredStartupPermissions(permissionStatus({ [key]: false }), {
          enabled: true,
        }),
      ).toBe(true);
    },
  );

  it("反向诊断标记不应阻塞启动", () => {
    expect(
      hasRequiredStartupPermissions(
        permissionStatus({
          backgroundRestricted: true,
          requiresManualPowerManagement: true,
        }),
        { enabled: true },
      ),
    ).toBe(true);
  });

  it.each(["notification", "ring", "storage", "battery"] as const)(
    "硬前置权限 %s 缺失时阻塞启动",
    (key) => {
      expect(
        hasRequiredStartupPermissions(permissionStatus({ [key]: false }), {
          enabled: true,
        }),
      ).toBe(false);
    },
  );

  it("通知监听器必须显式启用", () => {
    expect(
      hasRequiredStartupPermissions(permissionStatus(), { enabled: false }),
    ).toBe(false);
    expect(hasRequiredStartupPermissions(permissionStatus(), null)).toBe(false);
    expect(
      hasRequiredStartupPermissions(permissionStatus(), { enabled: "true" }),
    ).toBe(false);
  });
});
