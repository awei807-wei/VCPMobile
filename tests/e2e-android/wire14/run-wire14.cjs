"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { ensureSingleDevice } = require("../scripts/adb-env.cjs");
const { connectAndroidCdp } = require("./android-cdp.cjs");
const { createScenarioFixture } = require("./fixture.cjs");
const {
  startDesktopServices,
  stopDesktopServices,
} = require("./desktop-server.cjs");
const {
  reconnectAndroidCdp,
  snapshotTopics,
  stopSync,
} = require("./android-state.cjs");
const { runAllScenarios } = require("./scenarios.cjs");
const { isSuccess, runSyncAttempt } = require("./sync-attempt.cjs");
const { auditEvidencePayload, summarizeScenarios } = require("./evidence.cjs");
const { snapshotFixtureState } = require("./desktop-state.cjs");

const DEFAULT_TIMEOUT_MS = 180_000;
const DEFAULT_POLL_MS = 500;
const DEFAULT_SCALE_TOPICS = 1794;
const ANDROID_EMULATOR_HOST = "10.0.2.2";
const TEMP_PROFILE_ID = "lan";
const SCENARIO_NAMES = [
  "owner_topic_identity",
  "bidirectional_messages",
  "tombstones",
  "avatars_and_attachments",
  "stop_and_bounded_recovery",
  "android_lifecycle",
  "scale_1794_topics",
  "final_noop_convergence",
];

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function usage() {
  return [
    "用法: node tests/e2e-android/wire14/run-wire14.cjs --desktop-root <VCPChat>",
    "",
    "选项:",
    "  --desktop-root <path>   Linux VCPChat 源码根目录（必填）",
    "  --timeout-ms <number>   每次同步最大等待时间（默认 180000）",
    "  --poll-ms <number>      同步状态轮询间隔（默认 500）",
    "  --scale-topics <number> 规模 fixture topic 数（默认 1794）",
    "  --evidence <path>       写入脱敏 JSON 证据",
    "  --help                  显示帮助",
  ].join("\n");
}

function positiveInteger(value, name) {
  if (!/^\d+$/.test(value)) throw new Error(`${name} 必须是正整数`);
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} 必须是正整数`);
  }
  return parsed;
}

function parseArgs(argv) {
  const options = {
    desktopRoot: null,
    timeoutMs: DEFAULT_TIMEOUT_MS,
    pollMs: DEFAULT_POLL_MS,
    scaleTopics: DEFAULT_SCALE_TOPICS,
    evidencePath: null,
    help: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--help" || argument === "-h") {
      options.help = true;
      continue;
    }
    if (
      ![
        "--desktop-root",
        "--timeout-ms",
        "--poll-ms",
        "--scale-topics",
        "--evidence",
      ].includes(argument)
    ) {
      throw new Error(`未知参数: ${argument}`);
    }
    const value = argv[index + 1];
    if (typeof value !== "string" || value.startsWith("--")) {
      throw new Error(`${argument} 缺少参数值`);
    }
    index += 1;
    if (argument === "--desktop-root")
      options.desktopRoot = path.resolve(value);
    else if (argument === "--timeout-ms")
      options.timeoutMs = positiveInteger(value, argument);
    else if (argument === "--poll-ms")
      options.pollMs = positiveInteger(value, argument);
    else if (argument === "--scale-topics")
      options.scaleTopics = positiveInteger(value, argument);
    else options.evidencePath = path.resolve(value);
  }
  if (!options.help && !options.desktopRoot) {
    throw new Error("--desktop-root 是必填参数");
  }
  return options;
}

function cloneJson(value) {
  return JSON.parse(JSON.stringify(value));
}

function profileWithRuntime(settings, profileId, wsUrl, httpUrl, token) {
  const profiles = Array.isArray(settings?.connectionProfiles)
    ? cloneJson(settings.connectionProfiles)
    : [];
  const existing = profiles.find((profile) => profile?.id === profileId);
  const profile = {
    ...(existing || {}),
    id: profileId,
    name: existing?.name || (profileId === "wan" ? "外网" : "内网"),
    syncServerUrl: wsUrl,
    syncHttpUrl: httpUrl,
    syncToken: token,
  };
  const nextProfiles = profiles.filter((item) => item?.id !== profileId);
  nextProfiles.push(profile);
  return nextProfiles;
}

function buildSettingsUpdate(settings, wsUrl, httpUrl, token) {
  const connectionProfiles = profileWithRuntime(
    settings,
    TEMP_PROFILE_ID,
    wsUrl,
    httpUrl,
    token,
  );
  return {
    connectionProfiles,
    activeConnectionProfileId: TEMP_PROFILE_ID,
    syncServerUrl: wsUrl,
    syncHttpUrl: httpUrl,
    syncToken: token,
  };
}

function selectedFixtureTopics(fixture) {
  return fixture.owners.flatMap((owner) => {
    const selected = [];
    if (owner.ownerId === fixture.ids.ownerA) {
      selected.push(
        fixture.ids.sharedTopic,
        fixture.ids.linuxTopic,
        fixture.ids.androidTopic,
        fixture.ids.messageTombstoneTopic,
        fixture.ids.topicTombstoneTopic,
        fixture.ids.attachmentTopic,
        fixture.ids.lifecycleTopic,
      );
    } else if (owner.ownerId === fixture.ids.ownerB) {
      selected.push(fixture.ids.sharedTopic);
    } else if (owner.ownerId === fixture.ids.groupOwner) {
      selected.push(fixture.ids.sharedTopic);
    } else if (owner.ownerId === fixture.ids.scaleOwner && owner.topics[0]) {
      selected.push(owner.topics[0].id);
    }
    return selected.map((topicId) => ({
      ownerType: owner.ownerType,
      ownerId: owner.ownerId,
      topicId,
    }));
  });
}

async function prepareHarness(options) {
  const tempRoot = await fs.mkdtemp(
    path.join(os.tmpdir(), "vcpmobile-wire14-e2e-"),
  );
  const appDataPath = path.join(tempRoot, "AppData");
  const token = crypto.randomBytes(32).toString("hex");
  const fixtureRunId = crypto.randomBytes(8).toString("hex");
  let desktop = null;
  let cdp = null;
  let originalSettings = null;
  let settingsChanged = false;
  try {
    const fixture = await createScenarioFixture(appDataPath, {
      scaleTopics: options.scaleTopics,
      runId: fixtureRunId,
    });
    desktop = await startDesktopServices({
      desktopRoot: options.desktopRoot,
      appDataPath,
      token,
    });
    ensureSingleDevice();
    cdp = await connectAndroidCdp();
    originalSettings = cloneJson(await cdp.cdp.invoke("read_settings"));
    const wsUrl = `ws://${ANDROID_EMULATOR_HOST}:${desktop.wsPort}`;
    const httpUrl = `http://${ANDROID_EMULATOR_HOST}:${desktop.httpPort}`;
    await cdp.cdp.invoke("update_settings", {
      updates: buildSettingsUpdate(originalSettings, wsUrl, httpUrl, token),
    });
    settingsChanged = true;
    const storedAttachment = await cdp.cdp.invoke("store_file", {
      originalName: "wire14-e2e-synthetic.txt",
      fileBytes: [...fixture.assets.attachmentBytes],
      mimeType: "text/plain",
    });
    if (storedAttachment?.hash !== fixture.assets.attachmentHash) {
      throw new Error("Android 本地 CAS 预置结果与内容哈希不一致");
    }
    await cdp.cdp.installEventBuffer();
    return {
      tempRoot,
      fixture,
      desktop,
      cdp,
      originalSettings,
      settingsChanged,
      abortController: new AbortController(),
    };
  } catch (error) {
    let restoreError = null;
    if (settingsChanged) {
      const recoveryRuntime = { cdp, originalSettings, settingsChanged };
      try {
        await restoreSettings(recoveryRuntime);
        cdp = recoveryRuntime.cdp;
      } catch (cause) {
        restoreError = cause;
      }
    }
    await cdp?.close().catch(() => {});
    if (desktop) await stopDesktopServices(desktop).catch(() => {});
    await fs.rm(tempRoot, { recursive: true, force: true }).catch(() => {});
    if (restoreError) {
      throw new Error("E2E 初始化失败，且 Android 原始配置恢复失败", {
        cause: new AggregateError([error, restoreError]),
      });
    }
    throw error;
  }
}

async function restoreSettings(runtime) {
  if (!runtime?.originalSettings) {
    if (runtime?.settingsChanged) throw new Error("缺少 Android 原始配置快照");
    return;
  }
  if (runtime.cdp?.cdp) {
    try {
      await runtime.cdp.cdp.invoke("write_settings", {
        settings: runtime.originalSettings,
      });
      runtime.settingsChanged = false;
      return;
    } catch {
      await runtime.cdp.close().catch(() => {});
      runtime.cdp = null;
    }
  }
  runtime.cdp = await reconnectAndroidCdp();
  await runtime.cdp.cdp.invoke("write_settings", {
    settings: runtime.originalSettings,
  });
  runtime.settingsChanged = false;
}

async function cleanupHarness(runtime) {
  const issues = [];
  if (!runtime) return issues;
  runtime.abortController?.abort();
  try {
    if (runtime.cdp?.cdp && (await runtime.cdp.cdp.invoke("is_sync_active"))) {
      const stopped = await stopSync(runtime.cdp.cdp);
      if (!stopped) issues.push("sync_stop_timeout");
    }
  } catch {
    issues.push("sync_stop_failed");
  }
  try {
    await restoreSettings(runtime);
  } catch {
    issues.push("settings_restore_failed");
  }
  try {
    await runtime.cdp?.close();
  } catch {
    issues.push("cdp_close_failed");
  }
  try {
    await stopDesktopServices(runtime.desktop);
  } catch {
    issues.push("desktop_stop_failed");
  }
  try {
    await fs.rm(runtime.tempRoot, { recursive: true, force: true });
  } catch {
    issues.push("temporary_directory_cleanup_failed");
  }
  return issues;
}

async function runInitialAttempts(runtime, options, attempts) {
  const first = await runSyncAttempt(runtime, "first_sync", options);
  attempts.push(first);
  if (!isSuccess(first)) return { first, second: null, initialOk: false };

  runtime.initialSnapshot = await snapshotTopics(
    runtime.cdp.cdp,
    runtime.fixture.owners,
    selectedFixtureTopics(runtime.fixture),
  );
  const desktopBefore = await snapshotFixtureState(runtime);
  const transportBefore = runtime.desktop.snapshotTransportCounters();
  await sleep(options.pollMs);
  const second = await runSyncAttempt(runtime, "second_sync", options);
  attempts.push(second);
  const secondSnapshot = await snapshotTopics(
    runtime.cdp.cdp,
    runtime.fixture.owners,
    selectedFixtureTopics(runtime.fixture),
  );
  const desktopAfter = await snapshotFixtureState(runtime);
  const transportAfter = runtime.desktop.snapshotTransportCounters();
  const secondNoOp = Boolean(
    isSuccess(second) &&
    JSON.stringify(runtime.initialSnapshot) ===
      JSON.stringify(secondSnapshot) &&
    JSON.stringify(desktopBefore) === JSON.stringify(desktopAfter) &&
    transportAfter.operations === transportBefore.operations &&
    transportAfter.httpRequests === transportBefore.httpRequests,
  );
  runtime.secondConvergence = {
    noOp: secondNoOp,
    transportOperations: transportAfter.operations - transportBefore.operations,
    httpRequests: transportAfter.httpRequests - transportBefore.httpRequests,
    androidStateUnchanged:
      JSON.stringify(runtime.initialSnapshot) ===
      JSON.stringify(secondSnapshot),
    desktopStateUnchanged:
      JSON.stringify(desktopBefore) === JSON.stringify(desktopAfter),
  };
  return {
    first,
    second,
    initialOk: secondNoOp,
  };
}

function failedScenarioRecords(reason) {
  return SCENARIO_NAMES.map((name) => ({ name, status: "failed", reason }));
}

function expectedFailureObserved(attempt) {
  return Boolean(
    typeof attempt?.expectedFailure === "string" &&
    attempt?.started &&
    attempt.timedOut !== true &&
    attempt.terminalStatus === "error" &&
    attempt.handshakeObserved === true &&
    attempt.finalAckObserved === false &&
    Array.isArray(attempt.errorCodes) &&
    attempt.errorCodes.includes(attempt.expectedFailure),
  );
}

function attemptMeetsGate(attempt) {
  return attempt?.expectedFailure
    ? expectedFailureObserved(attempt)
    : isSuccess(attempt);
}

function allAttemptsMeetGate(attempts) {
  return attempts.length > 0 && attempts.every(attemptMeetsGate);
}

function firstFailure(attempts, scenarios) {
  const failedAttempt = attempts.find((attempt) => !attemptMeetsGate(attempt));
  if (failedAttempt) return `attempt_${failedAttempt.name}`;
  const failedScenario = scenarios.find(
    (scenario) => scenario.status !== "passed",
  );
  return failedScenario ? `scenario_${failedScenario.name}` : null;
}

function buildResult(
  options,
  runtime,
  attempts,
  scenarioResults,
  cleanupIssues,
  initial,
) {
  const scenarioSummary = summarizeScenarios(scenarioResults);
  const protocolAttemptsOk = allAttemptsMeetGate(attempts);
  const scenarioGate = Boolean(
    initial?.initialOk &&
    protocolAttemptsOk &&
    scenarioSummary.count === SCENARIO_NAMES.length &&
    scenarioSummary.passed === SCENARIO_NAMES.length &&
    cleanupIssues.length === 0,
  );
  const result = {
    schema: "vcp.android.wire14.e2e.v2",
    ok: false,
    protocol: {
      wire: "1.4",
      plugin: runtime?.desktop?.manifest?.version || "1.4.0",
      mode: "centralIndex",
      androidHost: ANDROID_EMULATOR_HOST,
    },
    fixture: {
      synthetic: true,
      totalTopics: runtime?.fixture?.totalTopics || 0,
      scaleTopics: options.scaleTopics,
      tokenOutput: null,
      messageOutput: null,
      piiOutput: null,
      pathOutput: null,
    },
    attempts,
    scenarios: scenarioSummary,
    hardGate: {
      ok: false,
      initialOk: initial?.initialOk === true,
      secondNoOp: runtime?.secondConvergence || null,
      protocolAttemptsOk,
      requiredScenarioCount: SCENARIO_NAMES.length,
    },
    cleanup: {
      ok: cleanupIssues.length === 0,
      issueCount: cleanupIssues.length,
      issues: cleanupIssues,
    },
    privacy: null,
    failure: scenarioGate
      ? null
      : firstFailure(attempts, scenarioSummary.records),
  };
  const privacy = auditEvidencePayload(result, [runtime?.desktop?.token]);
  result.fixture.tokenOutput = privacy.tokenOutput;
  result.fixture.messageOutput = privacy.messageOutput;
  result.fixture.piiOutput = privacy.piiOutput;
  result.fixture.pathOutput = privacy.pathOutput;
  result.privacy = privacy;
  const hardGate = scenarioGate && privacy.ok;
  result.ok = hardGate;
  result.hardGate.ok = hardGate;
  if (!privacy.ok) result.failure = "evidence_privacy_scan_failed";
  return result;
}

async function writeEvidence(filePath, result) {
  if (!filePath) return;
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, `${JSON.stringify(result, null, 2)}\n`, "utf8");
}

async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  if (options.help) {
    process.stdout.write(`${usage()}\n`);
    return 0;
  }

  const previousLog = console.log;
  const previousWarn = console.warn;
  const previousError = console.error;
  console.log = () => {};
  console.warn = () => {};
  console.error = () => {};
  let runtime = null;
  const attempts = [];
  let scenarioResults = [];
  let initial = null;
  let cleanupIssues = [];
  let result;
  try {
    runtime = await prepareHarness(options);
    initial = await runInitialAttempts(runtime, options, attempts);
    if (initial.initialOk) {
      scenarioResults = await runAllScenarios({
        runtime,
        fixture: runtime.fixture,
        options,
        attempts,
        initialSnapshot: runtime.initialSnapshot,
      });
    } else {
      scenarioResults = failedScenarioRecords("initial_sync_failed");
    }
  } catch {
    if (scenarioResults.length === 0)
      scenarioResults = failedScenarioRecords("harness_setup_failed");
    if (!initial) initial = { initialOk: false };
  } finally {
    cleanupIssues = await cleanupHarness(runtime);
    result = buildResult(
      options,
      runtime,
      attempts,
      scenarioResults,
      cleanupIssues,
      initial,
    );
    console.log = previousLog;
    console.warn = previousWarn;
    console.error = previousError;
  }
  await writeEvidence(options.evidencePath, result);
  process.stdout.write(`${JSON.stringify(result)}\n`);
  return result.ok ? 0 : 1;
}

if (require.main === module) {
  main()
    .catch(() => {
      process.stdout.write(
        `${JSON.stringify({
          schema: "vcp.android.wire14.e2e.v2",
          ok: false,
          failure: "invalid_arguments_or_runtime_error",
        })}\n`,
      );
      process.exitCode = 1;
    })
    .then((code) => {
      if (Number.isInteger(code)) process.exitCode = code;
    });
}

module.exports = {
  ANDROID_EMULATOR_HOST,
  SCENARIO_NAMES,
  attemptMeetsGate,
  buildSettingsUpdate,
  expectedFailureObserved,
  parseArgs,
  main,
};
