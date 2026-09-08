"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const {
  DEBUG_PACKAGE,
  SSE_PATHS,
  assertDebugPackage,
  createMessageId,
  identityFromSearchResult,
  resolveIdentityInput,
} = require("./helper-stream-e2e-support.cjs");
const { startLoopbackSseServer } = require("./helper-stream-e2e-server.cjs");
const {
  sanitizeChannelEvents,
  sanitizeEvidence,
} = require("./helper-stream-e2e-evidence.cjs");
const {
  channelInvokeExpression,
} = require("./helper-stream-e2e-channel.cjs");
const {
  cleanupResources,
  parseArgs,
} = require("./helper-stream-e2e-runner.cjs");

const ROOT = path.resolve(__dirname, "..", "..");

async function readChunk(reader) {
  const result = await reader.read();
  assert.equal(result.done, false);
  return Buffer.from(result.value).toString("utf8");
}

test("loopback SSE 按 A/B 顺序发送，并在客户端取消后记录 cancellation", async () => {
  const server = await startLoopbackSseServer();
  const controller = new AbortController();
  let reader;
  try {
    const response = await fetch(`${server.baseUrl}${SSE_PATHS[0]}`, {
      method: "POST",
      headers: {
        authorization: "Bearer android-helper-e2e-secret",
        accept: "text/event-stream",
        "content-type": "application/json",
      },
      body: JSON.stringify({
        requestId: "android-helper-e2e-request",
        messages: [{ role: "user", content: "secret prompt" }],
        model: "android-helper-e2e",
        stream: true,
      }),
      signal: controller.signal,
    });
    assert.equal(response.status, 200);
    reader = response.body.getReader();
    assert.match(await readChunk(reader), /"A"/);
    const accepted = await server.waitFor("accepted", 1_000);
    assert.equal(accepted.path, SSE_PATHS[0]);
    assert.equal(server.releaseB(), true);
    assert.match(await readChunk(reader), /"B"/);
    await reader.cancel();
    await server.waitFor("cancelled", 1_000);
    assert.equal(server.isCancelled(), true);
    const contract = server.snapshotContracts()[0];
    assert.equal(contract.headers.authorizationPresent, true);
    assert.equal(contract.body.stream, true);
    assert.equal(JSON.stringify(contract).includes("android-helper-e2e-secret"), false);
    assert.equal(JSON.stringify(contract).includes("secret prompt"), false);
  } finally {
    controller.abort();
    await server.close();
  }
});

test("参数、Debug 包和完整复合身份校验均 fail-closed", () => {
  assert.equal(parseArgs([]).searchQuery, "全");
  assert.equal(parseArgs(["--no-launch"]).launch, false);
  assert.deepEqual(
    resolveIdentityInput({
      identityJson: JSON.stringify({ ownerType: "group", ownerId: "g-1", topicId: "t-1" }),
    }),
    { ownerType: "group", ownerId: "g-1", topicId: "t-1" },
  );
  assert.deepEqual(
    identityFromSearchResult({ results: [{ owner_type: "agent", owner_id: "a-1", topic_id: "t-1" }] }),
    { ownerType: "agent", ownerId: "a-1", topicId: "t-1" },
  );
  assert.throws(
    () => resolveIdentityInput({ ownerType: "agent", ownerId: "a-1" }),
    (error) => error.code === "IDENTITY_INCOMPLETE",
  );
  assert.throws(
    () => resolveIdentityInput({ ownerType: "user", ownerId: "u-1", topicId: "t-1" }),
    (error) => error.code === "IDENTITY_INVALID",
  );
  assert.throws(
    () => assertDebugPackage("com.vcp.avatar.release"),
    (error) => error.code === "DEBUG_PACKAGE_REQUIRED",
  );
  assert.match(createMessageId(() => Buffer.from("0123456789abcdef01234567", "hex")), /^android-helper-e2e-[0-9a-f]{24}$/);
});

test("evidence 与 Channel 事件只保留白名单，不泄露身份、正文或 key", () => {
  const secret = "owner-secret-8f4c";
  const evidence = sanitizeEvidence({
    ok: true,
    packageDebug: DEBUG_PACKAGE,
    identity: {
      ownerType: "agent",
      ownerId: secret,
      topicId: "topic-secret",
      ownerIdPresent: true,
      topicIdPresent: true,
      allFieldsMatch: true,
    },
    server: {
      contracts: [{
        method: "POST",
        path: SSE_PATHS[0],
        headers: { authorizationPresent: true, contentTypePresent: true, acceptSsePresent: true },
        body: { validJson: true, stream: true, requestIdPresent: true, messagesArray: true, messageCount: 1, modelPresent: true },
        apiKey: "secret-key",
        prompt: "secret prompt",
      }],
    },
    phases: { resume: { result: { status: "completed", fullContent: "secret response" } } },
  });
  const events = sanitizeChannelEvents([
    {
      type: "aurora",
      messageId: "message-secret",
      ownerType: "agent",
      generation: 7,
      hasChunk: true,
      final: false,
    },
  ], { ownerType: "agent", ownerId: secret, topicId: "topic-secret" }, 7);
  const serialized = JSON.stringify({ evidence, events });
  assert.equal(serialized.includes(secret), false);
  assert.equal(serialized.includes("topic-secret"), false);
  assert.equal(serialized.includes("secret-key"), false);
  assert.equal(serialized.includes("secret prompt"), false);
  assert.equal(serialized.includes("secret response"), false);
  assert.equal(serialized.includes("message-secret"), false);
  assert.equal(evidence.schema, "vcp.android.helper.stream.e2e.v1");
  assert.equal(evidence.fixture.resetAndReinjectB4Required, true);
  assert.equal(events[0].generationMatches, true);
  assert.equal(events[0].messageIdMatches, false);
});

test("cleanupResources finally 只移除本次精确 reverse，并清理回调、CDP、server", async () => {
  const calls = [];
  const context = {
    cdp: {
      cdp: {
        async evaluate(expression) {
          calls.push(["evaluate", expression]);
          return true;
        },
      },
      async close() {
        calls.push(["cdp-close"]);
      },
    },
    callbacksInstalled: true,
    reverse: { created: true, localPort: 38_417 },
    server: {
      async close() {
        calls.push(["server-close"]);
      },
    },
  };
  const adbCalls = [];
  const issues = await cleanupResources(context, {
    runAdb(args, options) {
      adbCalls.push({ args, options });
    },
  });
  assert.deepEqual(issues, []);
  assert.deepEqual(adbCalls, [{
    args: ["reverse", "--remove", "tcp:38417"],
    options: { allowFailure: true },
  }]);
  assert.equal(adbCalls.some(({ args }) => args.includes("--remove-all")), false);
  assert.equal(context.callbacksRemoved, true);
  assert.equal(context.reverseRemoved, true);
  assert.equal(context.serverStopped, true);
  assert.deepEqual(calls.map(([name]) => name), ["evaluate", "cdp-close", "server-close"]);
});

test("cleanupResources 每个阶段失败仍继续清理后续资源并精确报告", async () => {
  const stages = ["callbacks", "cdp", "reverse", "server"];
  for (const failedStage of stages) {
    const calls = [];
    const context = {
      cdp: {
        cdp: {
          async evaluate() {
            calls.push("callbacks");
            if (failedStage === "callbacks") throw new Error("callback failure");
            return true;
          },
        },
        async close() {
          calls.push("cdp");
          if (failedStage === "cdp") throw new Error("cdp failure");
        },
      },
      callbacksInstalled: true,
      reverse: { created: true, localPort: 38_417 },
      server: {
        async close() {
          calls.push("server");
          if (failedStage === "server") throw new Error("server failure");
        },
      },
    };
    const adbCalls = [];
    const issues = await cleanupResources(context, {
      runAdb(args, options) {
        calls.push("reverse");
        adbCalls.push({ args, options });
        if (failedStage === "reverse") throw new Error("reverse failure");
      },
    });

    assert.deepEqual(issues, [
      failedStage === "callbacks" ? "cdp_callbacks" :
        failedStage === "cdp" ? "cdp_close" :
          failedStage === "reverse" ? "adb_reverse" : "sse_server",
    ]);
    assert.deepEqual(calls, ["callbacks", "cdp", "reverse", "server"]);
    assert.equal(context.serverStopped, failedStage === "server" ? undefined : true);
    assert.equal(context.callbacksRemoved, failedStage === "callbacks" ? undefined : true);
    assert.equal(adbCalls.length, 1);
  }
});

test("cleanupResources 没有 SSE server 时不伪造 serverStopped", async () => {
  const context = {
    cdp: { cdp: { async evaluate() { return true; } }, async close() {} },
    callbacksInstalled: true,
  };
  const issues = await cleanupResources(context);
  assert.deepEqual(issues, []);
  assert.equal(Object.prototype.hasOwnProperty.call(context, "serverStopped"), false);
});

test("runner 使用真实四个 IPC 命令和 transformCallback Channel，且不启动 release/helper service", () => {
  const runner = fs.readFileSync(path.join(ROOT, "tests/e2e-android/helper-stream-e2e-runner.cjs"), "utf8");
  const channel = fs.readFileSync(path.join(ROOT, "tests/e2e-android/helper-stream-e2e-channel.cjs"), "utf8");
  const flow = fs.readFileSync(path.join(ROOT, "tests/e2e-android/helper-stream-e2e-flow.cjs"), "utf8");
  const support = fs.readFileSync(path.join(ROOT, "tests/e2e-android/helper-stream-e2e-support.cjs"), "utf8");
  const adb = fs.readFileSync(path.join(ROOT, "tests/e2e-android/scripts/adb-env.cjs"), "utf8");
  const source = `${runner}\n${channel}\n${flow}\n${support}\n${adb}`;
  for (const command of ["sendToVCP", "recover_active_generation", "resume_stream", "interruptRequest"]) {
    assert.equal(source.includes(`"${command}"`), true, `缺少 ${command}`);
  }
  assert.equal(source.includes("transformCallback"), true);
  assert.equal(source.includes("__TAURI_TO_IPC_KEY__"), true);
  assert.equal(/am\s+force-stop/.test(source), false);
  assert.equal(source.includes("start_streaming_service"), false);
  assert.equal(source.includes(DEBUG_PACKAGE), true);
});

test("Channel expression 通过 Tauri 的 __CHANNEL__ 序列化传入 streamChannel", () => {
  const expression = channelInvokeExpression(
    "resume",
    "resume_stream",
    { msgId: "message-id", lastEventIndex: -1 },
    { messageId: "message-id", ownerType: "agent", ownerId: "owner-id", topicId: "topic-id" },
  );
  assert.match(expression, /transformCallback/);
  assert.match(expression, /__CHANNEL__:/);
  assert.match(expression, /streamChannel/);
  assert.match(expression, /resume_stream/);
  assert.equal(expression.includes("message-id"), true);
});
