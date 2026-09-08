"use strict";

const { makeFailure } = require("./b4-search-perf-metrics.cjs");
const { coldRestartGateFailures } = require("./b4-search-perf-gates.cjs");

function recordIndexFailure(state, phase, validation) {
  if (!validation.healthy) {
    state.failures.push(makeFailure(phase, validation.reason));
  }
}

function recordSearchFailures(state, options) {
  if (state.warm.warmups.some((sample) => !sample.ok)) {
    state.failures.push(makeFailure("warmup", "暖查询预热失败"));
  }
  if (state.warm.samples.some((sample) => !sample.ok)) {
    state.failures.push(makeFailure("warm", "暖查询计时样本失败"));
  }
  if (!state.warm.queryValidation?.valid) {
    for (const failure of state.warm.queryValidation?.failures || []) {
      state.failures.push(makeFailure("query_semantics", failure));
    }
  }
  if (state.cold.samples.length !== options.samples) {
    state.failures.push(makeFailure("cold", "冷进程样本未全部完成"));
  }
  if (state.cold.samples.some((sample) => !sample.ok)) {
    state.failures.push(makeFailure("cold", "冷进程首次查询样本失败"));
  }
  state.failures.push(...coldRestartGateFailures(state.cold.samples));
}

module.exports = { recordIndexFailure, recordSearchFailures };
