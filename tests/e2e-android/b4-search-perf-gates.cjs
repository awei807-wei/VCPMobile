"use strict";

function isPid(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function coldRestartSampleChecks(sample) {
  const previousPid = sample?.previousPid;
  const nextPid = sample?.nextPid;
  const cdpPid = sample?.cdpPid;
  return {
    processRestarted: sample?.processRestarted === true,
    foregrounded: sample?.foregrounded === true,
    previousPidChanged:
      isPid(previousPid) && isPid(nextPid) && previousPid !== nextPid,
    nextPidMatchesCdpPid:
      isPid(nextPid) && isPid(cdpPid) && nextPid === cdpPid,
  };
}

function coldRestartChecks(samples) {
  const list = Array.isArray(samples) ? samples : [];
  const sampleChecks = list.map(coldRestartSampleChecks);
  return {
    coldSamplesProcessRestarted:
      list.length > 0 && sampleChecks.every((checks) => checks.processRestarted),
    coldSamplesForegrounded:
      list.length > 0 && sampleChecks.every((checks) => checks.foregrounded),
    coldSamplesPreviousPidChanged:
      list.length > 0 &&
      sampleChecks.every((checks) => checks.previousPidChanged),
    coldSamplesNextPidMatchesCdpPid:
      list.length > 0 &&
      sampleChecks.every((checks) => checks.nextPidMatchesCdpPid),
  };
}

function coldRestartGateFailures(samples) {
  const list = Array.isArray(samples) ? samples : [];
  const failures = [];
  for (let index = 0; index < list.length; index += 1) {
    const sample = list[index];
    const checks = coldRestartSampleChecks(sample);
    const label = `冷进程样本 #${sample?.index ?? index + 1}`;
    if (!checks.processRestarted) {
      failures.push({
        phase: "cold_restart",
        message: `${label} processRestarted 必须为 true`,
      });
    }
    if (!checks.foregrounded) {
      failures.push({
        phase: "cold_restart",
        message: `${label} foregrounded 必须为 true`,
      });
    }
    if (!checks.previousPidChanged) {
      failures.push({
        phase: "cold_restart",
        message:
          `${label} previousPid 必须与 nextPid 不同 ` +
          `(previousPid=${sample?.previousPid ?? "null"}, nextPid=${sample?.nextPid ?? "null"})`,
      });
    }
    if (!checks.nextPidMatchesCdpPid) {
      failures.push({
        phase: "cold_restart",
        message:
          `${label} nextPid 必须与 cdpPid 匹配 ` +
          `(nextPid=${sample?.nextPid ?? "null"}, cdpPid=${sample?.cdpPid ?? "null"})`,
      });
    }
  }
  return failures;
}

function mergeColdRestartGateFailures(existing, samples) {
  const failures = Array.isArray(existing) ? [...existing] : [];
  for (const failure of coldRestartGateFailures(samples)) {
    if (!failures.some((item) => item.phase === failure.phase && item.message === failure.message)) {
      failures.push(failure);
    }
  }
  return failures;
}

module.exports = {
  coldRestartChecks,
  coldRestartGateFailures,
  coldRestartSampleChecks,
  mergeColdRestartGateFailures,
};
