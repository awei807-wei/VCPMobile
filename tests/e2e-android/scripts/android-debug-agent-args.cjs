'use strict';

const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');
const { ROOT } = require('./adb-env.cjs');

const DEFAULT_LOG_LINES = 80;
const MAX_LOG_LINES = 200;

function usage() {
  console.log(`VCPMobile Android Debug Agent CLI

Usage:
  pnpm android:debug:<command> -- [options]
  pnpm android:debug -- <command> [options]

Commands:
  doctor       Check adb, the selected USB device and local tool prerequisites
  dev          Run foreground USB/HMR development with bounded console output
  status       Print a compact device, WebView, package and tunnel snapshot
  logs         Print PID-scoped Debug app logcat without clearing device logs
  snapshot     Save status + bounded logs, optionally one screenshot
  screenshot   Save exactly one screenshot and print only its path and size
  reload       Relaunch only com.vcp.avatar.debug after tunnel readiness checks
  stop         Ask the active dev supervisor to stop and clean owned tunnels
  install      Verify an APK application id, then install only the Debug package
  grant        Best-effort runtime grants for only the Debug package

Options:
  --serial <id>       Required when more than one adb device is connected
  --json              Machine-readable stdout (dev uses NDJSON events)
  --lines <1..200>    Log line budget; default 80
  --level <v|i|w|e>   Android log priority; default i
  --screenshot        Include one screenshot in snapshot
  --out <path>        Snapshot directory or screenshot file under repo or /tmp
  --name <slug>       Screenshot filename slug
  --apk <path>        APK path for install
  --reset-data        Clear Debug app data after a verified Debug APK install

Safety boundary:
  This CLI never accepts a package override or Release mode. It never clears
  global logcat, never removes all adb reverse mappings, and never manipulates
  com.vcp.avatar.
`);
}

function requireValue(tokens, index, option) {
  const value = tokens[index];
  if (!value || value.startsWith('--')) {
    throw new Error(`${option} requires a value`);
  }
  return value;
}

function parseArgs(argv) {
  const tokens = [...argv];
  const command = tokens[0] && !tokens[0].startsWith('-') ? tokens.shift() : 'help';
  const options = {
    serial: '',
    json: false,
    lines: DEFAULT_LOG_LINES,
    level: 'i',
    screenshot: false,
    out: '',
    name: '',
    apk: '',
    resetData: false,
  };

  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (token === '--') continue;
    if (token === '--serial') options.serial = requireValue(tokens, ++index, token);
    else if (token === '--json') options.json = true;
    else if (token === '--lines') options.lines = Number(requireValue(tokens, ++index, token));
    else if (token === '--level') options.level = requireValue(tokens, ++index, token).toLowerCase();
    else if (token === '--screenshot') options.screenshot = true;
    else if (token === '--out') options.out = requireValue(tokens, ++index, token);
    else if (token === '--name') options.name = requireValue(tokens, ++index, token);
    else if (token === '--apk') options.apk = requireValue(tokens, ++index, token);
    else if (token === '--reset-data') options.resetData = true;
    else if (token === '--help' || token === '-h') return { command: 'help', options };
    else throw new Error(`Unknown option: ${token}`);
  }

  if (!Number.isInteger(options.lines) || options.lines < 1 || options.lines > MAX_LOG_LINES) {
    throw new Error(`--lines must be an integer between 1 and ${MAX_LOG_LINES}`);
  }
  if (!['v', 'i', 'w', 'e'].includes(options.level)) {
    throw new Error('--level must be one of v, i, w or e');
  }
  if (options.serial) process.env.ANDROID_SERIAL = options.serial;
  return { command, options };
}

function emit(options, event, message, details = undefined) {
  if (options.json) {
    console.log(JSON.stringify({ event, message, ...(details === undefined ? {} : { details }) }));
    return;
  }
  console.log(`[android-debug:${event}] ${message}`);
}

function printPayload(options, payload) {
  if (options.json) {
    console.log(JSON.stringify(payload));
    return;
  }
  for (const [key, value] of Object.entries(payload)) {
    const rendered = typeof value === 'object' && value !== null ? JSON.stringify(value) : String(value);
    console.log(`${key}=${rendered}`);
  }
}

function stripAnsi(value) {
  return value
    .replace(/\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g, '')
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/\r/g, '\n');
}

function resolveOutputPath(candidate, fallback) {
  const resolved = path.resolve(ROOT, candidate || fallback);
  const repoPrefix = `${ROOT}${path.sep}`;
  const tmpPrefix = `${path.resolve(os.tmpdir())}${path.sep}`;
  if (resolved !== ROOT && !resolved.startsWith(repoPrefix) && !resolved.startsWith(tmpPrefix)) {
    throw new Error(`Output path must stay under ${ROOT} or ${os.tmpdir()}`);
  }
  return resolved;
}

function packageManagerName() {
  return process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm';
}

function commandVersion(command, args) {
  const result = spawnSync(command, args, { cwd: ROOT, encoding: 'utf8', stdio: 'pipe' });
  return result.status === 0 ? (result.stdout || '').trim() : null;
}

module.exports = {
  commandVersion,
  emit,
  packageManagerName,
  parseArgs,
  printPayload,
  resolveOutputPath,
  stripAnsi,
  usage,
};
