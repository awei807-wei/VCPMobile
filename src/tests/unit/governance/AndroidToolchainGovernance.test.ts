import { describe, expect, it } from "vitest";

import ciWorkflow from "../../../../.github/workflows/ci.yml?raw";
import releaseWorkflow from "../../../../.github/workflows/release.yml?raw";
import androidSettingsGenerator from "../../../../.github/generate-tauri-android-settings.mjs?raw";
import packageManifestSource from "../../../../package.json?raw";

const IMMUTABLE_ACTION = /^[^@\s]+@[0-9a-f]{40}$/;

const workflowActions = (source: string) =>
  Array.from(
    source.matchAll(/^\s*uses:\s*([^\s#]+)(?:\s+#.*)?$/gm),
    (match) => match[1],
  );

describe("Android toolchain governance", () => {
  it("pins CI and release toolchains to exact reproducible versions", () => {
    for (const workflow of [ciWorkflow, releaseWorkflow]) {
      expect(workflow).toContain("runs-on: ubuntu-22.04");
      expect(workflow).toContain('NODE_VERSION: "22.23.2"');
      expect(workflow).toContain('PNPM_VERSION: "10.15.0"');
      expect(workflow).toContain('JAVA_VERSION: "17.0.20+8"');
      expect(workflow).toContain('RUST_VERSION: "1.97.1"');
      expect(workflow).toContain('ANDROID_COMPILE_SDK_VERSION: "36"');
      expect(workflow).toContain(
        'sdkmanager "platforms;android-${ANDROID_COMPILE_SDK_VERSION}"',
      );

      const actions = workflowActions(workflow);
      expect(actions.length).toBeGreaterThan(0);
      for (const action of actions) {
        expect(action).toMatch(IMMUTABLE_ACTION);
      }
    }

    expect(ciWorkflow).toContain('ANDROID_BUILD_TOOLS_VERSION: "36.0.0"');
    expect(releaseWorkflow).toContain('ANDROID_BUILD_TOOLS_VERSION: "37.0.0"');
    expect(releaseWorkflow).toContain(
      "build-tools/${ANDROID_BUILD_TOOLS_VERSION}/apksigner",
    );
  });

  it("keeps fork release, ACL and 16KB verification entrypoints intact", () => {
    const scripts = JSON.parse(packageManifestSource).scripts as Record<
      string,
      string
    >;

    expect(scripts["qa:regression"]).toBe("bash qa/regression.sh");
    expect(scripts["qa:robustness"]).toBe("bash qa/robustness.sh");
    expect(scripts["android:build:phone"]).toBe(
      "bash scripts/build_android_phone.sh",
    );
    expect(scripts["android:verify:phone"]).toContain(
      "scripts/verify_android_apk.py",
    );
    expect(releaseWorkflow).toContain("max-page-size=16384");
    expect(releaseWorkflow).toContain("scripts/verify_android_apk.py");
    expect(releaseWorkflow).toContain("--expected-abi arm64-v8a");
    expect(releaseWorkflow).toContain(
      "APK certificate does NOT match keystore",
    );
  });

  it("generates Tauri Android module settings from locked Cargo metadata", () => {
    expect(androidSettingsGenerator).toMatch(
      /"metadata",\s*"--locked",\s*"--manifest-path"/,
    );
    expect(androidSettingsGenerator).toContain("metadata.resolve?.root");
    expect(androidSettingsGenerator).toContain("tauri.build.gradle.kts");
    expect(androidSettingsGenerator).not.toMatch(/\/home\/|[A-Za-z]:\\\\/);
  });
});
