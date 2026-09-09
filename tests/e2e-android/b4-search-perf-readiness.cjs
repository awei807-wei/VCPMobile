"use strict";

const CORE_READY_TIMEOUT_MS = 30_000;
const CORE_READY_POLL_INTERVAL_MS = 200;
const CORE_STATUS_CDP_TIMEOUT_MS = 2_000;

function normalizeCoreStatus(value) {
  if (typeof value === "string") return value.trim().toLowerCase();
  if (!value || typeof value !== "object") return null;
  const candidates = [value.status, value.state, value.core];
  for (const candidate of candidates) {
    if (typeof candidate === "string") return candidate.trim().toLowerCase();
    if (candidate && typeof candidate === "object") {
      const nested = candidate.status || candidate.state;
      if (typeof nested === "string") return nested.trim().toLowerCase();
    }
  }
  return null;
}

function positiveInteger(value, fallback) {
  return Number.isFinite(value) && value > 0 ? Math.max(1, Math.floor(value)) : fallback;
}

function compactError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim().slice(0, 400) || "未知错误";
}

function coreReadyTimeout(timeoutMs, lastStatus) {
  const suffix = lastStatus ? `；最后状态=${lastStatus}` : "";
  const error = new Error(`核心 READY 等待超时（预算 ${timeoutMs}ms${suffix}）`);
  error.code = "CORE_READY_TIMEOUT";
  error.lastStatus = lastStatus;
  return error;
}

function unexpectedCoreStatus(status) {
  const value = status === null ? "null" : JSON.stringify(status);
  const error = new Error(`核心状态命令返回非预期状态：${value}`);
  error.code = "CORE_STATUS_UNEXPECTED";
  error.status = status;
  return error;
}

function coreStartupFailed(lastError) {
  const detail = typeof lastError === "string" && lastError.trim().length > 0
    ? `：${lastError.trim().slice(0, 400)}`
    : "";
  const error = new Error(`核心启动失败${detail}`);
  error.code = "CORE_STARTUP_FAILED";
  error.lastError = lastError ?? null;
  return error;
}

/**
 * Poll the lifecycle command until the core is ready within one bounded budget.
 * The clock and sleeper are injectable so tests never need wall-clock waiting.
 */
async function waitForCoreReady(cdp, options = {}) {
  if (!cdp || typeof cdp.invoke !== "function") {
    throw new TypeError("等待核心 READY 需要可用的 CDP invoke");
  }
  const timeoutMs = positiveInteger(options.timeoutMs, CORE_READY_TIMEOUT_MS);
  const pollIntervalMs = positiveInteger(
    options.pollIntervalMs,
    CORE_READY_POLL_INTERVAL_MS,
  );
  const statusTimeoutMs = positiveInteger(
    options.statusTimeoutMs,
    CORE_STATUS_CDP_TIMEOUT_MS,
  );
  const now = typeof options.now === "function" ? options.now : Date.now;
  const sleep =
    typeof options.sleep === "function"
      ? options.sleep
      : (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs));
  const startedAt = now();
  const deadline = startedAt + timeoutMs;
  let lastStatus = null;

  while (now() < deadline) {
    const remainingMs = Math.max(1, deadline - now());
    const status = await cdp.invoke(
      "get_core_status",
      {},
      { timeoutMs: Math.min(statusTimeoutMs, remainingMs) },
    );
    const normalized = normalizeCoreStatus(status);
    lastStatus = normalized;
    if (normalized === "ready") {
      return {
        status: normalized,
        elapsedMs: Math.max(0, now() - startedAt),
      };
    }
    if (normalized === "error") {
      let lastError;
      try {
        lastError = await cdp.invoke(
          "get_last_error",
          {},
          { timeoutMs: Math.min(statusTimeoutMs, Math.max(1, deadline - now())) },
        );
      } catch (error) {
        lastError = `读取 get_last_error 失败：${compactError(error)}`;
      }
      throw coreStartupFailed(lastError);
    }
    if (normalized !== "initializing" && normalized !== "optimizing") {
      throw unexpectedCoreStatus(normalized);
    }
    const afterPollMs = deadline - now();
    if (afterPollMs <= 0) break;
    await sleep(Math.min(pollIntervalMs, afterPollMs));
  }
  throw coreReadyTimeout(timeoutMs, lastStatus);
}

module.exports = {
  CORE_READY_POLL_INTERVAL_MS,
  CORE_READY_TIMEOUT_MS,
  CORE_STATUS_CDP_TIMEOUT_MS,
  normalizeCoreStatus,
  waitForCoreReady,
};
