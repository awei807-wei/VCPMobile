'use strict';

const path = require('path');
const {
  ROOT,
  DEBUG_PACKAGE,
  findAdb,
} = require('./adb-env.cjs');
const {
  commandVersion,
  parseArgs,
  printPayload,
  usage,
} = require('./android-debug-agent-args.cjs');
const {
  collectLogs,
  collectStatus,
  ensureUsbDevice,
  printStatus,
} = require('./android-debug-agent-status.cjs');
const {
  commandScreenshot,
  commandSnapshot,
} = require('./android-debug-agent-artifacts.cjs');
const {
  commandDev,
  commandReload,
  commandStop,
} = require('./android-debug-agent-dev.cjs');
const {
  commandGrant,
  commandInstall,
} = require('./android-debug-agent-install.cjs');

const SCHEMA_VERSION = 1;
const VITE_PORT = 1420;
const HMR_PORT = 1421;
const MAX_LOG_LINES = 200;
const HEARTBEAT_MS = 30_000;
const ARTIFACT_ROOT = path.join(ROOT, '.agent', 'android-debug');
const STATE_PATH = path.join(ARTIFACT_ROOT, 'dev-state.json');

const config = {
  SCHEMA_VERSION,
  VITE_PORT,
  HMR_PORT,
  HEARTBEAT_MS,
  MAX_LOG_LINES,
  ARTIFACT_ROOT,
  STATE_PATH,
};

// collectLogs keeps logcat PID-scoped with the exact `--pid=${status.app.pid}` argument.

async function commandDoctor(options) {
  const device = ensureUsbDevice();
  const status = await collectStatus(config);
  printPayload(options, {
    schema: `vcp.android-debug.doctor.v${SCHEMA_VERSION}`,
    ok: true,
    adb: findAdb(),
    pnpm: commandVersion(process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm', ['--version']),
    node: process.version,
    device: {
      serial: device.serial,
      model: device.model,
      api: device.sdk,
      abi: device.abi,
    },
    debugPackageInstalled: status.app.installed,
    devReady: status.dev.ready,
    artifactRoot: ARTIFACT_ROOT,
  });
  return 0;
}

async function commandStatus(options) {
  printStatus(options, await collectStatus(config));
  return 0;
}

async function commandLogs(options) {
  const result = collectLogs(options, await collectStatus(config), config);
  if (options.json) {
    console.log(JSON.stringify(result));
  } else {
    console.log(`package=${result.package} pid=${result.pid || '-'} level=${result.level} lines=${result.lines.length}/${result.limit}`);
    if (result.warning) console.log(`warning=${result.warning}`);
    for (const line of result.lines) console.log(line);
  }
  return result.pid ? 0 : 4;
}

async function main(argv = process.argv.slice(2)) {
  const { command, options } = parseArgs(argv);
  if (command === 'help') {
    usage();
    return 0;
  }
  if (command === 'doctor') return commandDoctor(options);
  if (command === 'dev') return commandDev(options, config);
  if (command === 'status') return commandStatus(options);
  if (command === 'logs') return commandLogs(options);
  if (command === 'snapshot') return commandSnapshot(options, config);
  if (command === 'screenshot') return commandScreenshot(options, config);
  if (command === 'reload') return commandReload(options, config);
  if (command === 'stop') return commandStop(options, config);
  if (command === 'install') return commandInstall(options, config);
  if (command === 'grant') return commandGrant(options);
  throw new Error(`Unknown command: ${command}`);
}

if (require.main === module) {
  Promise.resolve(main())
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error) => {
      const json = process.argv.includes('--json');
      if (json) {
        console.error(JSON.stringify({
          event: 'error',
          message: error instanceof Error ? error.message : String(error),
        }));
      } else {
        console.error(`[android-debug:error] ${error instanceof Error ? error.message : String(error)}`);
      }
      process.exitCode = 1;
    });
}

module.exports = { main };
