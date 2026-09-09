"use strict";

const http = require("node:http");
const net = require("node:net");
const path = require("node:path");
const { createRequire } = require("node:module");

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

function desktopPaths(desktopRoot) {
  return {
    plugin: path.join(
      desktopRoot,
      "VCPDistributedServer",
      "Plugin",
      "VCPMobileSync",
    ),
    cds: path.join(
      desktopRoot,
      "modules",
      "services",
      "chatDataService",
      "bin",
      `${process.platform}-${process.arch}`,
      process.platform === "win32"
        ? "vcp_chat_data_service.exe"
        : "vcp_chat_data_service",
    ),
  };
}

function assertDesktopSource(desktopRoot, paths) {
  const fs = require("node:fs");
  const required = [
    path.join(paths.plugin, "index.js"),
    path.join(paths.plugin, "plugin-manifest.json"),
    path.join(
      desktopRoot,
      "modules",
      "services",
      "chatDataService",
      "index.js",
    ),
    paths.cds,
  ];
  const missing = required.filter((filePath) => !fs.existsSync(filePath));
  if (missing.length > 0) {
    throw new Error(`桌面源码不完整（缺少 ${missing.length} 个运行时文件）`);
  }
}

function quietLogger() {
  return {
    info() {},
    warn() {},
    error() {},
  };
}

const NON_SKIP_ACTIONS = new Set([
  "PULL",
  "PUSH",
  "PULL_DELETE",
  "PUSH_DELETE",
]);

function parseWireFrame(data, strictParser) {
  try {
    const text = Buffer.isBuffer(data) ? data.toString("utf8") : String(data);
    const value = strictParser(text);
    return value && typeof value === "object" ? value : null;
  } catch {
    return null;
  }
}

function finalIdentity(frame, expectedType) {
  if (
    frame?.type !== expectedType ||
    frame?.phase !== "messages" ||
    !Number.isSafeInteger(frame?.sessionId) ||
    !Number.isSafeInteger(frame?.attemptId) ||
    typeof frame?.nonce !== "string" ||
    frame.nonce.length === 0
  ) {
    return null;
  }
  return `${frame.sessionId}\0${frame.attemptId}\0${frame.nonce}`;
}

function countWireOperations(value) {
  if (Array.isArray(value)) {
    return value.reduce((sum, item) => sum + countWireOperations(item), 0);
  }
  if (!value || typeof value !== "object") return 0;
  let count = NON_SKIP_ACTIONS.has(value.action) ? 1 : 0;
  if (Array.isArray(value.pullMessageIds)) count += value.pullMessageIds.length;
  if (Array.isArray(value.deleteMessages)) count += value.deleteMessages.length;
  if (value.pushTopic === true) count += 1;
  for (const [key, child] of Object.entries(value)) {
    if (
      ["action", "pullMessageIds", "deleteMessages", "pushTopic"].includes(key)
    )
      continue;
    count += countWireOperations(child);
  }
  return count;
}

function isFinalMessagesAck(frame) {
  return (
    frame?.type === "PHASE_ACK" &&
    frame?.phase === "messages" &&
    Number.isSafeInteger(frame?.sessionId) &&
    Number.isSafeInteger(frame?.attemptId) &&
    typeof frame?.nonce === "string" &&
    frame.nonce.length > 0
  );
}

function observeWireFrame(state, data, direction) {
  const frame = parseWireFrame(data, state.strictParser);
  if (!frame) return null;
  state.operations += countWireOperations(frame);
  if (frame.type === "SYNC_ENTITY_DELETE") state.operations += 1;
  if (direction === "mobile_to_desktop" && frame.type === "VERSION_CHECK") {
    state.versionChecksReceived += 1;
  }
  if (direction === "desktop_to_mobile" && frame.type === "VERSION_ACK") {
    state.versionAcksSent += 1;
  }
  if (direction === "desktop_to_mobile" && isFinalMessagesAck(frame)) {
    state.finalAcksSent += 1;
    state.finalAckIdentities.push(finalIdentity(frame, "PHASE_ACK"));
  }
  if (direction === "mobile_to_desktop") {
    const identity = finalIdentity(frame, "PHASE_COMPLETED");
    if (identity) {
      state.finalCompletionsReceived += 1;
      state.finalCompletionIdentities.push(identity);
    }
  }
  return frame;
}

function attachWireObserver(server, state) {
  if (!server || server.__wire14HarnessObserverInstalled) return;
  server.__wire14HarnessObserverInstalled = true;
  server.on("connection", (client) => {
    const originalSend = client.send.bind(client);
    client.send = (data, ...args) => {
      observeWireFrame(state, data, "desktop_to_mobile");
      return originalSend(data, ...args);
    };
    client.on("message", (data) => {
      const frame = observeWireFrame(state, data, "mobile_to_desktop");
      if (!state.connectionDropArmed || frame?.type !== "VERSION_CHECK") return;
      state.connectionDropArmed = false;
      state.connectionDropTriggered = true;
      setTimeout(() => client.terminate(), 50);
    });
  });
}

async function listenHttp(app, requestedPort = 0) {
  const server = http.createServer(app);
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(requestedPort, "0.0.0.0", resolve);
  });
  return { server, port: server.address().port };
}

async function closeHttp(server) {
  if (!server || !server.listening) return;
  await new Promise((resolve) => server.close(resolve));
}

async function startDesktopServices({
  desktopRoot,
  appDataPath,
  token,
  wsPort: requestedWsPort = 0,
  httpPort: requestedHttpPort = 0,
}) {
  const paths = desktopPaths(desktopRoot);
  assertDesktopSource(desktopRoot, paths);
  const previousAppDataEnv = process.env.VCPCHAT_APP_DATA_DIR;
  process.env.VCPCHAT_APP_DATA_DIR = appDataPath;
  const desktopRequire = createRequire(path.join(desktopRoot, "package.json"));
  const express = desktopRequire("express");
  const pluginLoggerPath = path.join(paths.plugin, "core", "logger.js");
  const pluginLogger = require(pluginLoggerPath);
  const { parseJsonWithoutDuplicateKeys } = require(
    path.join(paths.plugin, "protocol.js"),
  );
  const wireState = {
    httpRequests: 0,
    messagePushRequests: 0,
    operations: 0,
    versionChecksReceived: 0,
    versionAcksSent: 0,
    finalAcksSent: 0,
    finalCompletionsReceived: 0,
    finalAckIdentities: [],
    finalCompletionIdentities: [],
    strictParser: parseJsonWithoutDuplicateKeys,
    connectionDropArmed: false,
    connectionDropTriggered: false,
  };
  const wsState = pluginLogger.__wire14HarnessState || {
    server: null,
    observer: null,
  };
  wsState.observer = wireState;
  pluginLogger.__wire14HarnessState = wsState;
  if (!pluginLogger.__wire14HarnessSetWss) {
    const originalSetWss = pluginLogger.setWss;
    pluginLogger.setWss = (server) => {
      wsState.server = server;
      attachWireObserver(server, wsState.observer);
      return originalSetWss(server);
    };
    pluginLogger.__wire14HarnessSetWss = true;
  }
  const plugin = require(path.join(paths.plugin, "index.js"));
  const manifest = require(path.join(paths.plugin, "plugin-manifest.json"));
  const { ChatDataServiceFacade } = require(
    path.join(
      desktopRoot,
      "modules",
      "services",
      "chatDataService",
      "index.js",
    ),
  );
  if (manifest.version !== "1.4.0") {
    throw new Error("VCPMobileSync 插件版本不是 1.4.0");
  }

  const { stopWsServer } = require(
    path.join(paths.plugin, "transport", "websocket.js"),
  );
  const pluginDb = require(path.join(paths.plugin, "core", "db.js"));
  const { computeMessageFingerprint } = require(
    path.join(paths.plugin, "core", "hash.js"),
  );
  const wsPort = requestedWsPort || (await freePort());
  const facade = new ChatDataServiceFacade({
    appDataPath,
    binaryPath: paths.cds,
    enabled: true,
    notifyEnabled: false,
    tantivyEnabled: false,
    mobileSyncUseCentralIndex: true,
    logger: quietLogger(),
  });
  const client = await facade.startShadowMode();
  if (!client || !facade.client) {
    throw new Error("VCP-CDS 未能在隔离 AppData 中就绪");
  }

  const app = express();
  app.use((request, _response, next) => {
    wireState.httpRequests += 1;
    if (
      request.method === "POST" &&
      request.path === "/api/mobile-sync/messages/push"
    ) {
      wireState.messagePushRequests += 1;
    }
    next();
  });
  const projectBasePath = path.join(
    path.dirname(appDataPath),
    "VCPDistributedServer",
  );
  const pluginConfig = {
    MobileSyncToken: token,
    MobileSyncPort: String(wsPort),
    MobileSyncUseCentralIndex: true,
  };
  const services = { chatDataService: facade };
  let httpServer = null;
  const faultState = { finalAckDropped: false };
  try {
    await plugin.registerRoutes(app, pluginConfig, projectBasePath, services);
    const httpBinding = await listenHttp(app, requestedHttpPort);
    httpServer = httpBinding.server;
    const runtime = {
      manifest,
      facade,
      desktopRoot,
      appDataPath,
      token,
      computeMessageFingerprint,
      httpServer,
      httpPort: httpBinding.port,
      wsPort,
      stopWsServer,
      get finalAckDropped() {
        return faultState.finalAckDropped;
      },
      get connectionDropTriggered() {
        return wireState.connectionDropTriggered;
      },
      get wsServer() {
        return wsState.server;
      },
      dropActiveWebSockets() {
        const server = wsState.server;
        if (!server) return 0;
        let dropped = 0;
        for (const client of server.clients) {
          try {
            client.terminate();
            dropped += 1;
          } catch {}
        }
        return dropped;
      },
      armNextConnectionDrop() {
        if (!wsState.server) return false;
        wireState.connectionDropTriggered = false;
        wireState.connectionDropArmed = true;
        return true;
      },
      snapshotTransportCounters() {
        return {
          httpRequests: wireState.httpRequests,
          messagePushRequests: wireState.messagePushRequests,
          operations: wireState.operations,
        };
      },
      snapshotProtocolCounters() {
        return {
          versionChecksReceived: wireState.versionChecksReceived,
          versionAcksSent: wireState.versionAcksSent,
          finalAcksSent: wireState.finalAcksSent,
          finalCompletionsReceived: wireState.finalCompletionsReceived,
          finalAckIdentities: [...wireState.finalAckIdentities],
          finalCompletionIdentities: [...wireState.finalCompletionIdentities],
        };
      },
      armFinalAckDrop() {
        const server = wsState.server;
        if (!server) return false;
        let armed = true;
        let listenerInstalled = true;
        const removeListener = () => {
          if (!listenerInstalled) return;
          listenerInstalled = false;
          server.removeListener("connection", onConnection);
        };
        const wrapClient = (client) => {
          if (!client || client.__wire14FinalAckDropWrapped) return;
          const originalSend = client.send.bind(client);
          client.__wire14FinalAckDropWrapped = true;
          client.send = (data, ...args) => {
            const payload = armed
              ? parseWireFrame(data, wireState.strictParser)
              : null;
            const isFinalAck = isFinalMessagesAck(payload);
            if (!isFinalAck) return originalSend(data, ...args);
            armed = false;
            faultState.finalAckDropped = true;
            removeListener();
            const callback = args.find((value) => typeof value === "function");
            if (callback) queueMicrotask(() => callback());
            return undefined;
          };
        };
        const onConnection = (client) => wrapClient(client);
        server.on("connection", onConnection);
        for (const client of server.clients) wrapClient(client);
        return true;
      },
      restoreAppDataEnv() {
        if (previousAppDataEnv === undefined)
          delete process.env.VCPCHAT_APP_DATA_DIR;
        else process.env.VCPCHAT_APP_DATA_DIR = previousAppDataEnv;
      },
      closePluginResources() {
        pluginLogger.getLogger().endSession();
        const database = pluginDb.getDb();
        if (database && !database.open) return;
        database?.close();
      },
    };
    return runtime;
  } catch (error) {
    await stopWsServer();
    await facade.stop();
    pluginLogger.getLogger().endSession();
    const database = pluginDb.getDb();
    if (database?.open) database.close();
    if (previousAppDataEnv === undefined)
      delete process.env.VCPCHAT_APP_DATA_DIR;
    else process.env.VCPCHAT_APP_DATA_DIR = previousAppDataEnv;
    throw error;
  }
}

async function stopDesktopServices(runtime) {
  if (!runtime) return;
  let firstError = null;
  try {
    await closeHttp(runtime.httpServer);
  } catch (error) {
    firstError ||= error;
  }
  try {
    await runtime.stopWsServer();
  } catch (error) {
    firstError ||= error;
  }
  try {
    await runtime.facade.stop();
  } catch (error) {
    firstError ||= error;
  }
  try {
    runtime.closePluginResources();
  } catch (error) {
    firstError ||= error;
  }
  try {
    runtime.restoreAppDataEnv();
  } catch (error) {
    firstError ||= error;
  }
  if (firstError) throw firstError;
}

module.exports = {
  countWireOperations,
  startDesktopServices,
  stopDesktopServices,
};
