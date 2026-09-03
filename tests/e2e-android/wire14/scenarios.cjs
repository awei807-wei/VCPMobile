"use strict";

const { message, missingAttachment } = require("./fixture-data.cjs");
const {
  appendMessage,
  backgroundAndResume,
  deleteMessages,
  deleteTopic,
  forceStopAndReconnect,
  compactHash,
  loadTopicMessages,
  messageDigest,
  snapshotTopics,
  stopSync,
} = require("./android-state.cjs");
const {
  appendHistoryMessage,
  readHistory,
  reconcile,
  snapshotFixtureState,
  topicExists,
} = require("./desktop-state.cjs");
const { isSuccess, runSyncAttempt } = require("./sync-attempt.cjs");
const { inspectScaleFixture } = require("./scale-gate.cjs");

function owner(ownerId, ownerType = "agent") {
  return { ownerType, ownerId };
}

function selected(ownerId, topicId, ownerType = "agent") {
  return { ownerType, ownerId, topicId };
}

function scenarioMessageId(context, label) {
  const runId = context.fixture.runId || "static";
  return `wire14-${label}-${runId}`;
}

function staticFailure(name, reason) {
  return { name, status: "failed", reason };
}

const KNOWN_SCENARIO_FAILURES = new Map([
  [
    "cross owner identity was not isolated",
    "owner_topic_identity_not_isolated",
  ],
  ["desktop reconcile failed", "desktop_reconcile_failed"],
  ["Linux to Android sync failed", "linux_to_android_sync_failed"],
  ["Android to Linux sync failed", "android_to_linux_sync_failed"],
  [
    "bidirectional message was not observed",
    "bidirectional_message_not_observed",
  ],
  ["message tombstone sync failed", "message_tombstone_sync_failed"],
  ["topic tombstone sync failed", "topic_tombstone_sync_failed"],
  ["topic tombstone recheck failed", "topic_tombstone_recheck_failed"],
  ["tombstone item resurrected", "tombstone_item_resurrected"],
  [
    "message tombstone remained live on desktop",
    "message_tombstone_desktop_live",
  ],
  ["topic tombstone remained live on desktop", "topic_tombstone_desktop_live"],
  [
    "topic tombstone resurrected on desktop",
    "topic_tombstone_desktop_resurrected",
  ],
  ["topic tombstone remained live on Android", "topic_tombstone_android_live"],
  ["missing binary metadata sync failed", "missing_binary_sync_failed"],
  ["invalid attachment cleanup failed", "invalid_attachment_cleanup_failed"],
  ["attachment contract did not match", "attachment_contract_mismatch"],
  [
    "pulled attachment was not bound to verified CAS",
    "attachment_verified_cas_missing",
  ],
  [
    "pulled missing attachment was not metadata-only",
    "attachment_missing_not_metadata_only",
  ],
  ["avatar was not pulled", "avatar_not_pulled"],
  [
    "mobile missing attachment was not accepted",
    "mobile_missing_attachment_not_accepted",
  ],
  [
    "invalid attachment hash was not rejected",
    "invalid_attachment_hash_not_rejected",
  ],
  ["manual stop restart failed", "manual_stop_restart_failed"],
  ["bounded recovery was not proven", "bounded_recovery_not_proven"],
  ["background resume failed", "background_resume_failed"],
  ["force stop cold start failed", "force_stop_cold_start_failed"],
  ["scale equivalent did not reach 1794 topics", "scale_gate_failed"],
  ["final no-op convergence changed local state", "final_noop_changed_state"],
]);

async function scenario(name, operation) {
  try {
    return { name, status: "passed", ...(await operation()) };
  } catch (error) {
    const code =
      typeof error?.code === "string" && /^[A-Z0-9_:-]{1,80}$/.test(error.code)
        ? error.code.toLowerCase()
        : null;
    const knownReason = KNOWN_SCENARIO_FAILURES.get(error?.message);
    return staticFailure(
      name,
      code || knownReason || "scenario_execution_failed",
    );
  }
}

async function sync(context, name, hooks = {}) {
  const attempt = await runSyncAttempt(
    context.runtime,
    name,
    context.options,
    hooks,
  );
  context.attempts.push(attempt);
  return attempt;
}

async function runIdentityIsolation(context) {
  const snapshot = await snapshotTopics(
    context.runtime.cdp.cdp,
    [
      owner(context.fixture.ids.ownerA),
      owner(context.fixture.ids.ownerB),
      owner(context.fixture.ids.groupOwner, "group"),
    ],
    [
      selected(context.fixture.ids.ownerA, context.fixture.ids.sharedTopic),
      selected(context.fixture.ids.ownerB, context.fixture.ids.sharedTopic),
      selected(
        context.fixture.ids.groupOwner,
        context.fixture.ids.sharedTopic,
        "group",
      ),
    ],
  );
  const owners = [
    owner(context.fixture.ids.ownerA),
    owner(context.fixture.ids.ownerB),
    owner(context.fixture.ids.groupOwner, "group"),
  ];
  const actualMessages = await Promise.all(
    owners.map((item) =>
      loadTopicMessages(
        context.runtime.cdp.cdp,
        item.ownerType,
        item.ownerId,
        context.fixture.ids.sharedTopic,
      ),
    ),
  );
  const expectedMessages = owners.map(
    (item) =>
      context.fixture.histories.get(
        `${item.ownerId}\0${context.fixture.ids.sharedTopic}`,
      ) || [],
  );
  const actualHashes = actualMessages.map((messages) =>
    messages.map((item) => item?.content_hash || item?.contentHash || null),
  );
  const expectedHashes = expectedMessages.map((messages) =>
    messages.map((item) =>
      context.runtime.desktop.computeMessageFingerprint(item),
    ),
  );
  const crossOwnerIsolated =
    actualMessages.every((messages) => messages.length === 1) &&
    actualMessages.every(
      (messages, index) => messages[0]?.id === expectedMessages[index][0]?.id,
    ) &&
    actualHashes.every(
      (hashes, index) => hashes[0] === expectedHashes[index][0],
    ) &&
    new Set(actualHashes.flat()).size === owners.length;
  const groupAvatar = await context.runtime.cdp.cdp.invoke("get_avatar", {
    ownerType: "group",
    ownerId: context.fixture.ids.groupOwner,
  });
  const groupAvatarBytes =
    groupAvatar?.imageData || groupAvatar?.image_data || [];
  const groupAvatarObserved =
    (Array.isArray(groupAvatarBytes) ||
      groupAvatarBytes instanceof Uint8Array) &&
    groupAvatarBytes.length > 0;
  if (!crossOwnerIsolated || !groupAvatarObserved) {
    throw new Error("cross owner identity was not isolated");
  }
  return {
    crossOwnerIsolated,
    groupAvatarObserved,
    topicCount: snapshot.topicCount,
    messageCount: actualMessages.reduce(
      (sum, messages) => sum + messages.length,
      0,
    ),
    hashes: actualHashes.flat().map(compactHash),
  };
}

async function runBidirectional(context) {
  const linuxMessage = message(
    scenarioMessageId(context, "linux-to-android"),
    "Synthetic Linux to Android message.",
    1700001000001,
  );
  await appendHistoryMessage(
    context.runtime,
    context.fixture.ids.ownerA,
    context.fixture.ids.linuxTopic,
    linuxMessage,
  );
  await reconcile(
    context.runtime,
    context.fixture.ids.ownerA,
    context.fixture.ids.linuxTopic,
  );
  const pullAttempt = await sync(context, "linux_to_android");
  if (!isSuccess(pullAttempt)) throw new Error("Linux to Android sync failed");
  const pulled = await loadTopicMessages(
    context.runtime.cdp.cdp,
    "agent",
    context.fixture.ids.ownerA,
    context.fixture.ids.linuxTopic,
  );
  const linuxMessageHash =
    context.runtime.desktop.computeMessageFingerprint(linuxMessage);
  const androidObserved = pulled.some(
    (item) =>
      item?.id === linuxMessage.id &&
      (item?.content_hash || item?.contentHash) === linuxMessageHash,
  );

  const androidMessage = message(
    scenarioMessageId(context, "android-to-linux"),
    "Synthetic Android to Linux message.",
    1700001000002,
  );
  await appendMessage(
    context.runtime.cdp.cdp,
    "agent",
    context.fixture.ids.ownerB,
    context.fixture.ids.sharedTopic,
    androidMessage,
  );
  const pushAttempt = await sync(context, "android_to_linux");
  if (!isSuccess(pushAttempt)) throw new Error("Android to Linux sync failed");
  const desktopHistory = await readHistory(
    context.runtime,
    context.fixture.ids.ownerB,
    context.fixture.ids.sharedTopic,
  );
  const androidMessageHash =
    context.runtime.desktop.computeMessageFingerprint(androidMessage);
  const desktopObserved = desktopHistory.some(
    (item) =>
      item?.id === androidMessage.id &&
      context.runtime.desktop.computeMessageFingerprint(item) ===
        androidMessageHash,
  );
  if (!androidObserved || !desktopObserved)
    throw new Error("bidirectional message was not observed");
  return {
    androidObserved,
    desktopObserved,
    messageCount: desktopHistory.length,
  };
}

async function runTombstones(context) {
  await deleteMessages(
    context.runtime.cdp.cdp,
    "agent",
    context.fixture.ids.ownerA,
    context.fixture.ids.messageTombstoneTopic,
    [context.fixture.ids.messageTombstone],
  );
  const messageDeleteAttempt = await sync(context, "message_tombstone");
  if (!isSuccess(messageDeleteAttempt))
    throw new Error("message tombstone sync failed");
  const afterMessageDelete = await readHistory(
    context.runtime,
    context.fixture.ids.ownerA,
    context.fixture.ids.messageTombstoneTopic,
  );
  const messageStillLive = afterMessageDelete.some(
    (item) => item?.id === context.fixture.ids.messageTombstone,
  );

  await deleteTopic(
    context.runtime.cdp.cdp,
    "agent",
    context.fixture.ids.ownerA,
    context.fixture.ids.topicTombstoneTopic,
  );
  const topicDeleteAttempt = await sync(context, "topic_tombstone");
  if (!isSuccess(topicDeleteAttempt))
    throw new Error("topic tombstone sync failed");
  const desktopTopicStillLive = await topicExists(
    context.runtime,
    context.fixture.ids.ownerA,
    context.fixture.ids.topicTombstoneTopic,
  );
  const secondTopicDeleteAttempt = await sync(
    context,
    "topic_tombstone_recheck",
  );
  if (!isSuccess(secondTopicDeleteAttempt))
    throw new Error("topic tombstone recheck failed");
  const desktopTopicStillLiveAfterRecheck = await topicExists(
    context.runtime,
    context.fixture.ids.ownerA,
    context.fixture.ids.topicTombstoneTopic,
  );
  const androidTopics = await context.runtime.cdp.cdp.invoke("get_topics", {
    ownerId: context.fixture.ids.ownerA,
    ownerType: "agent",
  });
  const androidTopicStillLive =
    Array.isArray(androidTopics) &&
    androidTopics.some(
      (item) => item?.id === context.fixture.ids.topicTombstoneTopic,
    );
  if (messageStillLive)
    throw new Error("message tombstone remained live on desktop");
  if (desktopTopicStillLive)
    throw new Error("topic tombstone remained live on desktop");
  if (desktopTopicStillLiveAfterRecheck)
    throw new Error("topic tombstone resurrected on desktop");
  if (androidTopicStillLive)
    throw new Error("topic tombstone remained live on Android");
  const noResurrection = true;
  return {
    noResurrection,
    attemptCount: 3,
    messageCount: afterMessageDelete.length,
  };
}

async function runAttachments(context) {
  const baseline = await loadTopicMessages(
    context.runtime.cdp.cdp,
    "agent",
    context.fixture.ids.ownerA,
    context.fixture.ids.attachmentTopic,
  );
  const attachmentMessage = baseline.find(
    (item) => item?.id === context.fixture.ids.attachmentMessage,
  );
  const attachmentItems = Array.isArray(attachmentMessage?.attachments)
    ? attachmentMessage.attachments
    : [];
  const validAttachmentObserved = attachmentItems.some(
    (item) =>
      item?.hash === context.fixture.assets.attachmentHash &&
      item?.status === "ready" &&
      Boolean(item?.src || item?.internalPath),
  );
  const missingAttachmentObserved = attachmentItems.some(
    (item) =>
      item?.hash === context.fixture.assets.missingAttachmentHash &&
      item?.status === "desktop_only" &&
      !item?.src &&
      !item?.internalPath,
  );
  const validAndMissingMetadata =
    attachmentItems.length === 2 &&
    attachmentItems.every((item) => /^[a-f0-9]{64}$/i.test(item?.hash || "")) &&
    validAttachmentObserved &&
    missingAttachmentObserved;
  const avatar = await context.runtime.cdp.cdp.invoke("get_avatar", {
    ownerType: "agent",
    ownerId: context.fixture.ids.ownerA,
  });
  const avatarBytes = avatar?.imageData || avatar?.image_data || [];
  const avatarObserved =
    (Array.isArray(avatarBytes) || avatarBytes instanceof Uint8Array) &&
    avatarBytes.length > 0;
  const missingMessage = message(
    scenarioMessageId(context, "mobile-missing-binary"),
    "Synthetic mobile message with metadata-only attachment.",
    1700001100001,
    { attachments: [missingAttachment()] },
  );
  await appendMessage(
    context.runtime.cdp.cdp,
    "agent",
    context.fixture.ids.ownerA,
    context.fixture.ids.attachmentTopic,
    missingMessage,
  );
  const missingAttempt = await sync(context, "missing_binary_attachment");
  if (!isSuccess(missingAttempt))
    throw new Error("missing binary metadata sync failed");
  const desktopHistory = await readHistory(
    context.runtime,
    context.fixture.ids.ownerA,
    context.fixture.ids.attachmentTopic,
  );
  const desktopMissing = desktopHistory.find(
    (item) => item?.id === missingMessage.id,
  );
  const missingBinaryAccepted =
    desktopMissing?.attachments?.some(
      (item) =>
        (item?.hash || item?._fileManagerData?.hash) ===
          context.fixture.assets.missingAttachmentHash &&
        (!item.src || item.src === "") &&
        (!item.internalPath || item.internalPath === "") &&
        (!item?._fileManagerData?.internalPath ||
          item._fileManagerData.internalPath === ""),
    ) === true;

  let invalidRejected = false;
  let invalidCodeObserved = false;
  let invalidPushObserved = false;
  let invalidAttempt = null;
  const invalidMessage = message(
    `wire14-e2e-invalid-attachment-${context.fixture.runId || "static"}`,
    "Synthetic message for the Wire 1.4 invalid attachment boundary.",
    1700001100002,
  );
  await appendMessage(
    context.runtime.cdp.cdp,
    "agent",
    context.fixture.ids.ownerA,
    context.fixture.ids.androidTopic,
    invalidMessage,
  );
  await context.runtime.cdp.cdp.invoke(
    "debug_inject_invalid_attachment_for_wire14",
    {
      ownerType: "agent",
      ownerId: context.fixture.ids.ownerA,
      topicId: context.fixture.ids.androidTopic,
      msgId: invalidMessage.id,
      attachmentHash: context.fixture.assets.invalidAttachmentHash,
    },
  );
  const invalidTransportBefore =
    context.runtime.desktop.snapshotTransportCounters();
  invalidAttempt = await sync(context, "invalid_attachment_hash");
  invalidAttempt.expectedFailure = "MOBILE_ATTACHMENT_INVALID";
  const invalidTransportAfter =
    context.runtime.desktop.snapshotTransportCounters();
  const invalidTransportOperations =
    invalidTransportAfter.operations - invalidTransportBefore.operations;
  const invalidHttpRequests =
    invalidTransportAfter.httpRequests - invalidTransportBefore.httpRequests;
  const invalidMessagePushRequests =
    invalidTransportAfter.messagePushRequests -
    invalidTransportBefore.messagePushRequests;
  invalidCodeObserved = (invalidAttempt.errorCodes || []).includes(
    "MOBILE_ATTACHMENT_INVALID",
  );
  invalidPushObserved =
    invalidTransportOperations > 0 &&
    invalidHttpRequests > 0 &&
    invalidMessagePushRequests > 0;
  invalidRejected =
    invalidAttempt.terminalStatus === "error" &&
    invalidCodeObserved &&
    invalidPushObserved;
  await deleteMessages(
    context.runtime.cdp.cdp,
    "agent",
    context.fixture.ids.ownerA,
    context.fixture.ids.androidTopic,
    [invalidMessage.id],
  );
  await context.runtime.cdp.cdp.invoke("cleanup_single_orphaned_attachment", {
    hash: context.fixture.assets.invalidAttachmentHash,
  });
  const cleanup = await sync(context, "invalid_attachment_cleanup");
  if (!isSuccess(cleanup)) throw new Error("invalid attachment cleanup failed");
  if (!validAttachmentObserved)
    throw new Error("pulled attachment was not bound to verified CAS");
  if (!missingAttachmentObserved || !validAndMissingMetadata) {
    throw new Error("pulled missing attachment was not metadata-only");
  }
  if (!avatarObserved) throw new Error("avatar was not pulled");
  if (!missingBinaryAccepted)
    throw new Error("mobile missing attachment was not accepted");
  if (!invalidRejected)
    throw new Error("invalid attachment hash was not rejected");
  return {
    desktopObserved: true,
    androidObserved: validAndMissingMetadata,
    avatarObserved,
    validAttachmentObserved: Boolean(validAttachmentObserved),
    missingBinaryAccepted: Boolean(missingBinaryAccepted),
    invalidRejected,
    invalidCodeObserved,
    invalidMessagePushRequests,
    invalidPushObserved,
    messageCount: desktopHistory.length,
    errorCodes: invalidAttempt?.errorCodes || [],
  };
}

async function runStopAndRecovery(context) {
  await context.runtime.cdp.cdp.invoke("start_manual_sync");
  const activeBeforeStop =
    await context.runtime.cdp.cdp.invoke("is_sync_active");
  const stopped = await stopSync(context.runtime.cdp.cdp);
  const stoppedStatus = await context.runtime.cdp.cdp
    .invoke("get_sync_status")
    .catch(() => null);
  const restartAttempt = await sync(context, "manual_stop_restart");
  if (!activeBeforeStop || !stopped || !isSuccess(restartAttempt)) {
    throw new Error("manual stop restart failed");
  }

  const interruptionArmed = context.runtime.desktop.armNextConnectionDrop();
  const recoveryAttempt = await sync(
    context,
    "connection_interruption_recovery",
  );
  const droppedConnections = context.runtime.desktop.connectionDropTriggered
    ? 1
    : 0;
  const injected = interruptionArmed && droppedConnections === 1;
  const recovered = isSuccess(recoveryAttempt) && injected;
  const retryCount = recoveryAttempt?.events?.statusCounts?.retrying || 0;

  const finalAckInjectionAvailable = context.runtime.desktop.armFinalAckDrop();
  const finalAckAttempt = finalAckInjectionAvailable
    ? await sync(context, "final_ack_loss")
    : null;
  const finalAckDropped = context.runtime.desktop.finalAckDropped;
  if (finalAckAttempt) finalAckAttempt.expectedFailure = "FINAL_ACK_TIMEOUT";
  const finalAckRejected = Boolean(
    finalAckInjectionAvailable &&
    finalAckDropped &&
    finalAckAttempt?.terminalStatus === "error" &&
    (finalAckAttempt.errorCodes || []).includes("FINAL_ACK_TIMEOUT"),
  );
  const finalAckRecoveryAttempt = finalAckRejected
    ? await sync(context, "final_ack_loss_recovery")
    : null;
  const finalAckRecovered = isSuccess(finalAckRecoveryAttempt);
  if (
    !recovered ||
    retryCount < 1 ||
    retryCount > 3 ||
    !finalAckRejected ||
    !finalAckRecovered
  ) {
    throw new Error("bounded recovery was not proven");
  }
  return {
    stopped: activeBeforeStop && stopped && stoppedStatus === "disconnected",
    injected,
    droppedConnections,
    recovered,
    finalAckDropped,
    finalAckRejected,
    finalAckObserved: finalAckRecoveryAttempt?.finalAckObserved === true,
    retryCount,
  };
}

async function runLifecycle(context) {
  const lifecycle = await backgroundAndResume();
  const backgroundAttempt = await sync(context, "android_background_resume");
  if (
    !lifecycle.backgrounded ||
    !lifecycle.foregrounded ||
    !isSuccess(backgroundAttempt)
  ) {
    throw new Error("background resume failed");
  }
  const restart = await forceStopAndReconnect(context.runtime);
  const coldStartAttempt = await sync(context, "android_force_stop_cold_start");
  if (
    !restart.processRestarted ||
    !restart.foregrounded ||
    !isSuccess(coldStartAttempt)
  ) {
    throw new Error("force stop cold start failed");
  }
  return {
    backgrounded: lifecycle.backgrounded,
    foregrounded: lifecycle.foregrounded && restart.foregrounded,
    processRestarted: restart.processRestarted,
  };
}

async function runScale(context) {
  const scaleOwner = context.fixture.owners.find(
    (item) => item.ownerId === context.fixture.ids.scaleOwner,
  );
  const expectedTopicCount = scaleOwner?.topics.length || 0;
  const first = context.attempts.find(
    (attempt) => attempt.name === "first_sync",
  );
  const scaleState = await inspectScaleFixture(context);
  if (
    expectedTopicCount < 1794 ||
    scaleState.topicCount !== expectedTopicCount ||
    !scaleState.exactTopicSet ||
    !scaleState.canonicalHashesPresent ||
    !scaleState.exactMessages ||
    context.runtime.secondConvergence?.noOp !== true ||
    !isSuccess(first) ||
    !Number.isSafeInteger(first.peakAndroidMemoryBytes) ||
    !Number.isSafeInteger(first.peakDesktopMemoryBytes)
  ) {
    throw new Error("scale equivalent did not reach 1794 topics");
  }
  return {
    expectedTopicCount,
    topicCount: scaleState.topicCount,
    messageCount: scaleState.messageCount,
    exactTopicSet: scaleState.exactTopicSet,
    canonicalHashesPresent: scaleState.canonicalHashesPresent,
    exactMessages: scaleState.exactMessages,
    topicSetHash: scaleState.topicSetHash,
    topicStateHash: scaleState.topicStateHash,
    messageStateHash: scaleState.messageStateHash,
    secondNoOp: true,
    peakAndroidMemoryBytes: first.peakAndroidMemoryBytes,
    peakDesktopMemoryBytes: first.peakDesktopMemoryBytes,
  };
}

async function runFinalNoop(context) {
  const desktopBefore = await snapshotFixtureState(context.runtime);
  const owners = context.fixture.owners.map((item) =>
    owner(item.ownerId, item.ownerType),
  );
  const selectedTopics = [
    selected(context.fixture.ids.ownerA, context.fixture.ids.linuxTopic),
    selected(context.fixture.ids.ownerB, context.fixture.ids.sharedTopic),
    selected(
      context.fixture.ids.groupOwner,
      context.fixture.ids.sharedTopic,
      "group",
    ),
  ];
  const before = await snapshotTopics(
    context.runtime.cdp.cdp,
    owners,
    selectedTopics,
  );
  const transportBefore = context.runtime.desktop.snapshotTransportCounters();
  const attempt = await sync(context, "final_noop_convergence");
  const transportAfter = context.runtime.desktop.snapshotTransportCounters();
  const after = await snapshotTopics(
    context.runtime.cdp.cdp,
    owners,
    selectedTopics,
  );
  const desktopAfter = await snapshotFixtureState(context.runtime);
  const transportOperations =
    transportAfter.operations - transportBefore.operations;
  const httpRequests =
    transportAfter.httpRequests - transportBefore.httpRequests;
  const desktopUnchanged =
    JSON.stringify(desktopBefore) === JSON.stringify(desktopAfter);
  const noOp =
    isSuccess(attempt) &&
    before.topicCount === after.topicCount &&
    before.messageCount === after.messageCount &&
    JSON.stringify(before.owners) === JSON.stringify(after.owners) &&
    desktopUnchanged &&
    transportOperations === 0 &&
    httpRequests === 0;
  if (!noOp) throw new Error("final no-op convergence changed local state");
  return {
    noOp,
    desktopObserved: desktopUnchanged,
    topicCount: after.topicCount,
    messageCount: after.messageCount,
    transportOperations,
    httpRequests,
  };
}

async function runAllScenarios(context) {
  const results = [];
  results.push(
    await scenario("owner_topic_identity", () => runIdentityIsolation(context)),
  );
  results.push(
    await scenario("bidirectional_messages", () => runBidirectional(context)),
  );
  results.push(await scenario("tombstones", () => runTombstones(context)));
  results.push(
    await scenario("avatars_and_attachments", () => runAttachments(context)),
  );
  results.push(
    await scenario("stop_and_bounded_recovery", () =>
      runStopAndRecovery(context),
    ),
  );
  results.push(
    await scenario("android_lifecycle", () => runLifecycle(context)),
  );
  results.push(await scenario("scale_1794_topics", () => runScale(context)));
  results.push(
    await scenario("final_noop_convergence", () => runFinalNoop(context)),
  );
  return results;
}

module.exports = {
  runAllScenarios,
  runIdentityIsolation,
  runBidirectional,
  runTombstones,
  runAttachments,
  runStopAndRecovery,
  runLifecycle,
  runScale,
  runFinalNoop,
};
