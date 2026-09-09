"use strict";

const { codedError } = require("./helper-stream-e2e-support.cjs");

function channelCallbackExpression(expected) {
  const messageId = JSON.stringify(expected.messageId);
  const ownerType = JSON.stringify(expected.ownerType);
  const ownerId = JSON.stringify(expected.ownerId);
  const topicId = JSON.stringify(expected.topicId);
  return `const callbackId = internals.transformCallback((raw) => {
      const message = raw && typeof raw === 'object' &&
        Object.prototype.hasOwnProperty.call(raw, 'message') ? raw.message : raw;
      const context = message && typeof message.context === 'object' ? message.context : {};
      const finishReason = ['cancelled_by_user', 'completed', 'stop', 'error']
        .includes(message?.finishReason) ? message.finishReason : null;
      state.channels[channelName].push({
        type: typeof message?.type === 'string' ? message.type : 'unknown',
        messageIdMatches: message?.messageId === ${messageId},
        identityMatches: context.ownerType === ${ownerType} &&
          context.ownerId === ${ownerId} && context.topicId === ${topicId},
        ownerType: typeof context.ownerType === 'string' ? context.ownerType : null,
        generation: Number.isSafeInteger(message?.generation) ? message.generation : null,
        hasChunk: typeof message?.aurora?.chunk === 'string' && message.aurora.chunk.length > 0,
        final: Boolean(message?.finishReason || message?.error),
        finishReason,
      });
    });`;
}

function channelResultExpression(commandJson, argsJson) {
  return `const invocationArgs = { ...${argsJson}, streamChannel };
    state.promises[channelName] = internals.invoke(${commandJson}, invocationArgs)
      .then((value) => {
        const result = state.results[channelName];
        result.settled = true;
        result.ok = true;
        result.result = {
          status: typeof value?.status === 'string' ? value.status : null,
          finalization: value?.finalization === 'skipped' ? 'skipped' : null,
          finishReason: ['cancelled_by_user', 'completed', 'stop', 'error']
            .includes(value?.finishReason) ? value.finishReason : null,
          streamingStarted: typeof value?.streamingStarted === 'boolean' ? value.streamingStarted : null,
          hasFullContent: typeof value?.fullContent === 'string',
        };
        return value;
      }, () => {
        state.results[channelName] = { settled: true, ok: false, result: null };
        return null;
      });`;
}

function channelInvokeExpression(name, command, args, expected) {
  const nameJson = JSON.stringify(name);
  const commandJson = JSON.stringify(command);
  const argsJson = JSON.stringify(args);
  const callback = channelCallbackExpression(expected);
  const invocation = channelResultExpression(commandJson, argsJson);
  return `(async () => {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function' ||
        typeof internals.transformCallback !== 'function') {
      throw new Error('Tauri IPC/channel unavailable');
    }
    const state = window.__helperStreamE2E;
    const channelName = ${nameJson};
    if (!state || !state.channels || !state.callbackIds ||
        !state.promises || !state.results) {
      throw new Error('Tauri IPC/channel harness unavailable');
    }
    state.channels[channelName] = [];
    state.results[channelName] = { settled: false, ok: false, result: null };
    ${callback}
    state.callbackIds.push(callbackId);
    const streamChannel = {
      __TAURI_TO_IPC_KEY__: () => '__CHANNEL__:' + callbackId,
      toJSON: () => '__CHANNEL__:' + callbackId,
    };
    ${invocation}
    return { callbackId };
  })()`;
}

function installHarnessState(cdp) {
  return cdp.evaluate(`(() => {
    window.__helperStreamE2E = { channels: {}, callbackIds: [], promises: {}, results: {} };
    return true;
  })()`);
}

function cleanupHarnessStateExpression() {
  return `(() => {
    const state = window.__helperStreamE2E;
    const internals = window.__TAURI_INTERNALS__;
    if (!state || !internals) return false;
    for (const callbackId of state.callbackIds || []) {
      try { internals.unregisterCallback(callbackId); } catch {}
    }
    delete window.__helperStreamE2E;
    return true;
  })()`;
}

async function invokeChannel(cdp, name, command, args, expected) {
  return cdp.evaluate(channelInvokeExpression(name, command, args, expected));
}

async function readHarnessState(cdp) {
  return (await cdp.evaluate(`(() => {
    const state = window.__helperStreamE2E;
    if (!state) return null;
    return { channels: state.channels, results: state.results };
  })()`)) || { channels: {}, results: {} };
}

async function waitForState(cdp, predicate, timeoutMs, code) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const state = await readHarnessState(cdp);
    if (predicate(state)) return state;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw codedError(code, "Android helper 阶段超时");
}

module.exports = {
  channelInvokeExpression,
  cleanupHarnessStateExpression,
  installHarnessState,
  invokeChannel,
  readHarnessState,
  waitForState,
};
