'use strict';

const net = require('net');
const {
  DEBUG_PACKAGE,
  getDeviceInfo,
  runAdb,
} = require('./adb-env.cjs');
const { printPayload } = require('./android-debug-agent-args.cjs');

// 新版 adb（platform-tools ≥ 34）的 `reverse --list` 首列输出传输类型而非设备序列号。
const ADB_REVERSE_TRANSPORT_TOKENS = new Set(['usbffs', 'tcpip', 'local']);

function parseReverseMappings(output) {
  return output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const parts = line.split(/\s+/);
      if (parts.length >= 3) return { serial: parts[0], local: parts[1], remote: parts[2] };
      if (parts.length === 2) {
        return { serial: process.env.ANDROID_SERIAL || '', local: parts[0], remote: parts[1] };
      }
      return null;
    })
    .filter(Boolean);
}

function selectedReverseMappings(serial = getDeviceInfo().serial) {
  return parseReverseMappings(runAdb(['reverse', '--list'], { allowFailure: true }))
    .filter((mapping) => !mapping.serial
      || mapping.serial === serial
      || ADB_REVERSE_TRANSPORT_TOKENS.has(mapping.serial.toLowerCase()));
}

function ensureUsbDevice() {
  const device = getDeviceInfo();
  if (device.serial.includes(':')) {
    throw new Error(`USB transport required; selected adb serial looks network-based: ${device.serial}`);
  }
  return device;
}

function parseWmValue(output, label) {
  const override = output.match(new RegExp(`Override ${label}:\\s*([^\\r\\n]+)`));
  const physical = output.match(new RegExp(`Physical ${label}:\\s*([^\\r\\n]+)`));
  return (override || physical || [null, ''])[1].trim();
}

function parsePackageInfo(output) {
  const versionName = output.match(/versionName=([^\s]+)/)?.[1] || null;
  const versionCode = output.match(/versionCode=(\d+)/)?.[1] || null;
  return { versionName, versionCode };
}

function currentWebView() {
  const output = runAdb(['shell', 'dumpsys', 'webviewupdate'], { allowFailure: true });
  const match = output.match(/Current WebView package \(name, version\): \(([^,]+),\s*([^)]+)\)/);
  return match ? { provider: match[1], version: match[2] } : null;
}

function canConnect(port, timeoutMs = 350) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host: '127.0.0.1', port });
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(value);
    };
    socket.setTimeout(timeoutMs);
    socket.once('connect', () => finish(true));
    socket.once('timeout', () => finish(false));
    socket.once('error', () => finish(false));
  });
}

function collectPackageStatus() {
  const packagePath = runAdb(['shell', 'pm', 'path', DEBUG_PACKAGE], { allowFailure: true }).trim();
  const installed = packagePath.startsWith('package:');
  const packageDump = installed
    ? runAdb(['shell', 'dumpsys', 'package', DEBUG_PACKAGE], { allowFailure: true })
    : '';
  const pid = installed
    ? runAdb(['shell', 'pidof', '-s', DEBUG_PACKAGE], { allowFailure: true }).trim() || null
    : null;
  return { installed, ...parsePackageInfo(packageDump), pid };
}

function collectFocus() {
  const displayDump = runAdb(['shell', 'dumpsys', 'window', 'displays'], { allowFailure: true });
  const windowDump = displayDump.includes('mCurrentFocus=')
    ? displayDump
    : runAdb(['shell', 'dumpsys', 'window', 'windows'], { allowFailure: true });
  return windowDump.split(/\r?\n/).find((line) => line.includes('mCurrentFocus='))?.trim() || null;
}

function collectDisplayStatus() {
  const size = parseWmValue(runAdb(['shell', 'wm', 'size'], { allowFailure: true }), 'size');
  const densityText = parseWmValue(runAdb(['shell', 'wm', 'density'], { allowFailure: true }), 'density');
  const density = Number(densityText || '0') || null;
  const sizeMatch = size.match(/^(\d+)x(\d+)$/);
  const cssViewport = sizeMatch && density
    ? {
        width: Math.round((Number(sizeMatch[1]) * 160) / density),
        height: Math.round((Number(sizeMatch[2]) * 160) / density),
      }
    : null;
  return {
    pixels: size || null,
    density,
    cssViewport,
    fontScale: Number(runAdb(['shell', 'settings', 'get', 'system', 'font_scale'], { allowFailure: true }).trim() || '0') || null,
    navigationMode: runAdb(['shell', 'settings', 'get', 'secure', 'navigation_mode'], { allowFailure: true }).trim() || null,
  };
}

async function collectDevStatus(config, serial) {
  const mappings = selectedReverseMappings(serial);
  const [viteListening, hmrListening] = await Promise.all([
    canConnect(config.VITE_PORT),
    canConnect(config.HMR_PORT),
  ]);
  return {
    host: '127.0.0.1',
    vitePort: config.VITE_PORT,
    hmrPort: config.HMR_PORT,
    viteListening,
    hmrListening,
    reverse: mappings,
    ready: viteListening
      && mappings.some((item) => item.local === `tcp:${config.VITE_PORT}` && item.remote === `tcp:${config.VITE_PORT}`),
  };
}

async function collectStatus(config) {
  const device = getDeviceInfo();
  const app = collectPackageStatus();
  const focus = collectFocus();
  return {
    schema: `vcp.android-debug.status.v${config.SCHEMA_VERSION}`,
    generatedAt: new Date().toISOString(),
    package: DEBUG_PACKAGE,
    releasePackagePolicy: 'com.vcp.avatar is read-only and never manipulated',
    device: {
      serial: device.serial,
      manufacturer: device.manufacturer,
      model: device.model,
      android: device.release,
      api: device.sdk,
      abi: device.abi,
      transport: device.serial.includes(':') ? 'network' : 'usb',
    },
    display: collectDisplayStatus(),
    webView: currentWebView(),
    app: {
      ...app,
      foreground: Boolean(focus && focus.includes(DEBUG_PACKAGE)),
      focus,
    },
    dev: await collectDevStatus(config, device.serial),
  };
}

function printStatus(options, status) {
  if (options.json) {
    console.log(JSON.stringify(status));
    return;
  }
  printPayload(options, {
    schema: status.schema,
    package: status.package,
    device: `${status.device.serial} ${status.device.manufacturer} ${status.device.model}`,
    android: `${status.device.android} api=${status.device.api} abi=${status.device.abi}`,
    transport: status.device.transport,
    viewport: status.display.cssViewport
      ? `${status.display.cssViewport.width}x${status.display.cssViewport.height} density=${status.display.density} font=${status.display.fontScale}`
      : 'unknown',
    webview: status.webView ? `${status.webView.provider} ${status.webView.version}` : 'unknown',
    app: `installed=${status.app.installed} version=${status.app.versionName || '-'} pid=${status.app.pid || '-'} foreground=${status.app.foreground}`,
    dev: `ready=${status.dev.ready} vite=${status.dev.viteListening} hmr=${status.dev.hmrListening}`,
    reverse: status.dev.reverse,
  });
}

function collectLogs(options, status, config) {
  if (!status.app.pid) {
    return {
      schema: `vcp.android-debug.logs.v${config.SCHEMA_VERSION}`,
      package: DEBUG_PACKAGE,
      pid: null,
      level: options.level,
      limit: options.lines,
      lines: [],
      warning: 'Debug app process is not running; global logcat fallback is intentionally disabled',
    };
  }
  const output = runAdb([
    'logcat',
    '-d',
    `--pid=${status.app.pid}`,
    '-v',
    'threadtime',
    '-t',
    String(options.lines),
    `*:${options.level.toUpperCase()}`,
  ], { allowFailure: true, maxBuffer: 4 * 1024 * 1024 });
  return {
    schema: `vcp.android-debug.logs.v${config.SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    pid: status.app.pid,
    level: options.level,
    limit: options.lines,
    lines: output.split(/\r?\n/).filter(Boolean).slice(-options.lines),
  };
}

module.exports = {
  collectLogs,
  collectStatus,
  ensureUsbDevice,
  printStatus,
  selectedReverseMappings,
};
