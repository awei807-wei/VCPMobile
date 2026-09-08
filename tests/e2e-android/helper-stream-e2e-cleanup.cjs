"use strict";

const { removePreciseReverse } = require("./helper-stream-e2e-support.cjs");
const { cleanupHarnessStateExpression } = require("./helper-stream-e2e-channel.cjs");

async function cleanupResources(context, dependencies = {}) {
  const issues = [];
  if (context.cdp && context.callbacksInstalled) {
    try {
      const cdp = context.cdp.cdp;
      context.callbacksRemoved = await cdp.evaluate(cleanupHarnessStateExpression());
    } catch {
      issues.push("cdp_callbacks");
    }
  }
  try {
    await context.cdp?.close();
  } catch {
    issues.push("cdp_close");
  }
  if (context.reverse) {
    try {
      context.reverseRemoved = removePreciseReverse(context.reverse, dependencies);
    } catch {
      issues.push("adb_reverse");
    }
  }
  if (context.server && typeof context.server.close === "function") {
    try {
      await context.server.close();
      context.serverStopped = true;
    } catch {
      issues.push("sse_server");
    }
  }
  return issues;
}

module.exports = { cleanupResources };
