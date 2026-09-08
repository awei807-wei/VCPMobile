"use strict";

function compactError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim().slice(0, 400) || "未知错误";
}

function cleanupError(primary, failures) {
  if (failures.length === 0) return primary;
  const detail = failures
    .map((failure) => `${failure.phase}: ${failure.message}`)
    .join("；");
  const error = new Error(`${primary ? compactError(primary) + "；" : ""}清理失败：${detail}`);
  error.cause = primary || undefined;
  error.cleanupFailures = failures;
  const residualPaths = [
    ...(Array.isArray(primary?.fixturePayload?.residualPaths) ? primary.fixturePayload.residualPaths : []),
    ...failures.flatMap((failure) =>
      Array.isArray(failure.residualPaths) ? failure.residualPaths : []),
  ];
  // Fixture command failures are counted from cleanupFailures by the CLI;
  // attach the aggregate payload only for standalone cleanup commands, whose
  // caller has no other residual-path channel.
  if (residualPaths.length > 0 && !primary) {
    error.fixturePayload = {
      ...(primary?.fixturePayload || {}),
      residualPaths: [...new Set(residualPaths)],
    };
  }
  return error;
}

async function runCleanupActions(actions) {
  const failures = [];
  for (const [phase, action] of actions) {
    try {
      await action();
    } catch (error) {
      failures.push({
        phase,
        message: compactError(error),
        ...(Array.isArray(error?.residualPaths) ? { residualPaths: error.residualPaths } : {}),
      });
    }
  }
  return failures;
}

module.exports = { cleanupError, compactError, runCleanupActions };
