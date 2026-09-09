'use strict';

const fs = require('fs');
const path = require('path');
const { spawn, spawnSync } = require('child_process');
const {
  DEBUG_PACKAGE,
  ROOT,
  runAdb,
  timestamp,
  ensureDir,
} = require('./adb-env.cjs');
const {
  emit,
  packageManagerName,
  printPayload,
  stripAnsi,
} = require('./android-debug-agent-args.cjs');
const {
  collectStatus,
  ensureUsbDevice,
  selectedReverseMappings,
} = require('./android-debug-agent-status.cjs');

function setupReversePorts(config) {
  const existing = selectedReverseMappings();
  const owned = [];
  for (const port of [config.VITE_PORT, config.HMR_PORT]) {
    const local = `tcp:${port}`;
    const remote = `tcp:${port}`;
    const current = existing.find((mapping) => mapping.local === local);
    if (current) {
      if (current.remote !== remote) throw new Error(`adb reverse conflict: ${local} already maps to ${current.remote}`);
      continue;
    }
    runAdb(['reverse', local, remote]);
    owned.push(local);
  }
  return owned;
}

function cleanupReversePorts(ownedPorts) {
  for (const local of ownedPorts) runAdb(['reverse', '--remove', local], { allowFailure: true });
}

function writeState(state, statePath) {
  ensureDir(path.dirname(statePath));
  const tempPath = `${statePath}.tmp`;
  fs.writeFileSync(tempPath, `${JSON.stringify(state, null, 2)}\n`, 'utf8');
  fs.renameSync(tempPath, statePath);
}

function readState(statePath) {
  try {
    return JSON.parse(fs.readFileSync(statePath, 'utf8'));
  } catch {
    return null;
  }
}

function processAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function isExpectedSupervisor(state) {
  if (!state || !processAlive(state.supervisorPid)) return false;
  if (process.platform !== 'linux') return true;
  try {
    const cmdline = fs.readFileSync(`/proc/${state.supervisorPid}/cmdline`, 'utf8');
    return cmdline.includes('android-debug-agent.cjs') && cmdline.includes('dev');
  } catch {
    return false;
  }
}

function killChildTree(child, signal) {
  if (!child?.pid) return;
  try {
    if (process.platform === 'win32') {
      spawnSync('taskkill', ['/pid', String(child.pid), '/t'], { stdio: 'ignore' });
    } else {
      process.kill(-child.pid, signal);
    }
  } catch {
    // The child may already have exited.
  }
}

function prepareDev(options, config) {
  const device = ensureUsbDevice();
  const previous = readState(config.STATE_PATH);
  if (isExpectedSupervisor(previous)) {
    throw new Error(`Android Debug dev is already active under PID ${previous.supervisorPid}`);
  }
  if (previous) fs.rmSync(config.STATE_PATH, { force: true });

  const ownedPorts = setupReversePorts(config);
  const logPath = path.join(config.ARTIFACT_ROOT, 'dev-logs', `${timestamp()}.log`);
  ensureDir(path.dirname(logPath));
  const logStream = fs.createWriteStream(logPath, { flags: 'wx' });
  const environment = {
    ...process.env,
    ANDROID_SERIAL: device.serial,
    TAURI_DEV_HOST: '127.0.0.1',
    NO_COLOR: '1',
    CARGO_TERM_COLOR: 'never',
    GRADLE_OPTS: `${process.env.GRADLE_OPTS || ''} -Dorg.gradle.console=plain`.trim(),
  };
  const child = spawn(packageManagerName(), ['tauri', 'android', 'dev', '--host', '127.0.0.1'], {
    cwd: ROOT,
    env: environment,
    detached: process.platform !== 'win32',
    stdio: ['inherit', 'pipe', 'pipe'],
  });
  writeState({
    schema: `vcp.android-debug.dev-state.v${config.SCHEMA_VERSION}`,
    supervisorPid: process.pid,
    childPid: child.pid,
    serial: device.serial,
    package: DEBUG_PACKAGE,
    startedAt: new Date().toISOString(),
    logPath,
    ownedReversePorts: ownedPorts,
  }, config.STATE_PATH);
  return { child, device, logPath, logStream, ownedPorts };
}

function createDevConsumer(options, logStream) {
  const tail = [];
  const emitted = new Set();
  let errorCount = 0;
  const consume = (source, chunk) => {
    logStream.write(`[${source}] ${chunk}`);
    const lines = stripAnsi(String(chunk)).split(/\n+/).map((line) => line.trim()).filter(Boolean);
    for (const line of lines) {
      tail.push(line);
      if (tail.length > 60) tail.shift();
      const milestone = /VITE\s+v.*ready|Detected connected device|Finished .*target|Performing Streamed Install|^Success$|Starting: Intent/.test(line);
      const failure = /(^|\b)(error|failed|exception|panic)(:|\b)/i.test(line) && !/0 failed|without errors?/i.test(line);
      if (milestone && !emitted.has(line)) {
        emitted.add(line);
        emit(options, 'progress', line);
      } else if (failure && errorCount < 20) {
        errorCount += 1;
        emit(options, 'diagnostic', line);
      }
    }
  };
  return { consume, tail };
}

function waitForDev(options, config, context, consumer) {
  const { child, logPath, logStream, ownedPorts, tail } = { ...context, ...consumer };
  const started = Date.now();
  const heartbeat = setInterval(() => {
    emit(options, 'heartbeat', `running elapsed=${Math.round((Date.now() - started) / 1000)}s log=${logPath}`);
  }, config.HEARTBEAT_MS);
  heartbeat.unref();
  return new Promise((resolve) => {
    let finished = false;
    const finish = (code, reason, terminateChild) => {
      if (finished) return;
      finished = true;
      clearInterval(heartbeat);
      if (terminateChild) killChildTree(child, reason === 'SIGINT' ? 'SIGINT' : 'SIGTERM');
      cleanupReversePorts(ownedPorts);
      const current = readState(config.STATE_PATH);
      if (current?.supervisorPid === process.pid) fs.rmSync(config.STATE_PATH, { force: true });
      if (code !== 0) emit(options, 'failure-tail', `last ${Math.min(tail.length, 20)} lines are in ${logPath}`, tail.slice(-20));
      emit(options, 'exit', `code=${code} reason=${reason} log=${logPath}`);
      logStream.end();
      resolve(code);
    };
    child.once('error', (error) => {
      consumer.consume('spawn-error', error.message);
      finish(1, 'spawn-error', false);
    });
    child.once('exit', (code, signal) => finish(code ?? 1, signal || 'child-exit', false));
    process.once('SIGINT', () => finish(130, 'SIGINT', true));
    process.once('SIGTERM', () => finish(143, 'SIGTERM', true));
  });
}

async function commandDev(options, config) {
  const context = prepareDev(options, config);
  const consumer = createDevConsumer(options, context.logStream);
  context.child.stdout.on('data', (chunk) => consumer.consume('stdout', chunk));
  context.child.stderr.on('data', (chunk) => consumer.consume('stderr', chunk));
  emit(options, 'start', `serial=${context.device.serial} package=${DEBUG_PACKAGE}`, { logPath: context.logPath });
  emit(options, 'tunnel', `vite=${config.VITE_PORT} hmr=${config.HMR_PORT} owned=${context.ownedPorts.length}`);
  return waitForDev(options, config, context, consumer);
}

function resolveLauncherComponent() {
  const output = runAdb([
    'shell', 'cmd', 'package', 'resolve-activity', '--brief',
    '-a', 'android.intent.action.MAIN', '-c', 'android.intent.category.LAUNCHER',
    '--user', '0', DEBUG_PACKAGE,
  ], { allowFailure: true });
  const component = output.split(/\r?\n/).map((line) => line.trim()).findLast((line) => line.includes('/'));
  if (!component || !component.startsWith(`${DEBUG_PACKAGE}/`)) {
    throw new Error(`Unable to resolve Debug launcher component for ${DEBUG_PACKAGE}`);
  }
  return component;
}

async function commandReload(options, config) {
  ensureUsbDevice();
  const before = await collectStatus(config);
  if (!before.dev.viteListening || !before.dev.ready) {
    throw new Error('USB Dev is not ready; start pnpm android:debug:dev first');
  }
  const component = resolveLauncherComponent();
  runAdb(['shell', 'am', 'force-stop', DEBUG_PACKAGE]);
  runAdb(['shell', 'am', 'start', '-n', component]);
  await new Promise((resolve) => setTimeout(resolve, 1200));
  const after = await collectStatus(config);
  printPayload(options, {
    schema: `vcp.android-debug.reload.v${config.SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    component,
    pid: after.app.pid,
    foreground: after.app.foreground,
  });
  return after.app.foreground ? 0 : 5;
}

function commandStop(options, config) {
  const state = readState(config.STATE_PATH);
  if (!state) {
    emit(options, 'stop', 'no active dev state');
    return 0;
  }
  if (!isExpectedSupervisor(state)) {
    fs.rmSync(config.STATE_PATH, { force: true });
    emit(options, 'stop', 'removed stale state; reverse mappings were left untouched for safety');
    return 0;
  }
  process.kill(state.supervisorPid, 'SIGTERM');
  emit(options, 'stop', `requested supervisor PID ${state.supervisorPid} to stop`);
  return 0;
}

module.exports = {
  commandDev,
  commandReload,
  commandStop,
};
