'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawnSync } = require('child_process');
const {
  DEBUG_PACKAGE,
  runAdb,
} = require('./adb-env.cjs');
const {
  emit,
  printPayload,
  resolveOutputPath,
} = require('./android-debug-agent-args.cjs');
const { collectStatus, ensureUsbDevice } = require('./android-debug-agent-status.cjs');

function sdkRoots() {
  return [
    process.env.ANDROID_HOME,
    process.env.ANDROID_SDK_ROOT,
    process.platform === 'win32' && process.env.LOCALAPPDATA
      ? path.join(process.env.LOCALAPPDATA, 'Android', 'Sdk')
      : null,
    path.join(os.homedir(), 'Android', 'Sdk'),
    path.join(os.homedir(), 'Library', 'Android', 'sdk'),
  ].filter(Boolean);
}

function findAapt() {
  const executable = process.platform === 'win32' ? 'aapt.exe' : 'aapt';
  for (const sdk of sdkRoots()) {
    const buildTools = path.join(sdk, 'build-tools');
    if (!fs.existsSync(buildTools)) continue;
    const versions = fs.readdirSync(buildTools).sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
    for (const version of versions) {
      const candidate = path.join(buildTools, version, executable);
      if (fs.existsSync(candidate)) return candidate;
    }
  }
  throw new Error('aapt was not found; APK identity verification fails closed');
}

function verifyDebugApk(apkPath) {
  const aapt = findAapt();
  const result = spawnSync(aapt, ['dump', 'badging', apkPath], {
    encoding: 'utf8',
    stdio: 'pipe',
    maxBuffer: 4 * 1024 * 1024,
  });
  if (result.status !== 0) throw new Error(`aapt could not inspect APK: ${(result.stderr || '').trim()}`);
  const applicationId = (result.stdout || '').match(/^package:\s+name='([^']+)'/m)?.[1] || null;
  if (applicationId !== DEBUG_PACKAGE) {
    throw new Error(`Refusing APK application id ${applicationId || 'unknown'}; expected ${DEBUG_PACKAGE}`);
  }
  return { aapt, applicationId };
}

async function commandInstall(options, config) {
  ensureUsbDevice();
  if (!options.apk) throw new Error('install requires --apk <path>');
  const apkPath = resolveOutputPath(options.apk, '');
  if (!fs.existsSync(apkPath) || !fs.statSync(apkPath).isFile()) throw new Error(`APK does not exist: ${apkPath}`);
  const verified = verifyDebugApk(apkPath);
  emit(options, 'install', `verified ${verified.applicationId} via ${verified.aapt}`);
  runAdb(['install', '-r', '-d', apkPath], { stdio: 'inherit' });
  if (options.resetData) runAdb(['shell', 'pm', 'clear', DEBUG_PACKAGE]);
  const status = await collectStatus(config);
  printPayload(options, {
    schema: `vcp.android-debug.install.v${config.SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    apk: apkPath,
    versionName: status.app.versionName,
    resetData: options.resetData,
  });
  return 0;
}

function commandGrant(options) {
  const device = ensureUsbDevice();
  const permissions = [
    'android.permission.CAMERA',
    'android.permission.RECORD_AUDIO',
    'android.permission.ACCESS_FINE_LOCATION',
    'android.permission.ACCESS_COARSE_LOCATION',
    ...(device.sdk >= 33
      ? ['android.permission.POST_NOTIFICATIONS', 'android.permission.READ_MEDIA_IMAGES']
      : ['android.permission.READ_EXTERNAL_STORAGE']),
  ];
  for (const permission of permissions) runAdb(['shell', 'pm', 'grant', DEBUG_PACKAGE, permission], { allowFailure: true });
  runAdb(['shell', 'dumpsys', 'deviceidle', 'whitelist', `+${DEBUG_PACKAGE}`], { allowFailure: true });
  printPayload(options, {
    schema: 'vcp.android-debug.grant.v1',
    package: DEBUG_PACKAGE,
    attempted: permissions,
    manualOnly: ['notification-listener', 'oem-auto-start', 'battery-unrestricted', 'recents-lock'],
  });
  return 0;
}

module.exports = {
  commandGrant,
  commandInstall,
  findAapt,
  verifyDebugApk,
};
