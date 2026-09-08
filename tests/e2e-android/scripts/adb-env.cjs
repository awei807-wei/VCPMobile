const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync, spawnSync } = require('child_process');

const ROOT = path.resolve(__dirname, '..', '..', '..');
const DEBUG_PACKAGE = 'com.vcp.avatar.debug';
const MAX_ADB_ERROR_TEXT = 512;
let cachedAdb = null;

function isWindows() {
  return process.platform === 'win32';
}

function adbName() {
  return isWindows() ? 'adb.exe' : 'adb';
}

function pathEntries() {
  return (process.env.PATH || '').split(path.delimiter).filter(Boolean);
}

function candidateAdbPaths() {
  const candidates = [];
  for (const entry of pathEntries()) {
    candidates.push(path.join(entry, adbName()));
  }
  for (const key of ['ANDROID_HOME', 'ANDROID_SDK_ROOT']) {
    if (process.env[key]) {
      candidates.push(path.join(process.env[key], 'platform-tools', adbName()));
    }
  }
  if (isWindows() && process.env.LOCALAPPDATA) {
    candidates.push(path.join(process.env.LOCALAPPDATA, 'Android', 'Sdk', 'platform-tools', adbName()));
  }
  return candidates;
}

function findAdb() {
  if (cachedAdb) {
    return cachedAdb;
  }
  if (process.env.ADB) {
    cachedAdb = process.env.ADB;
    return cachedAdb;
  }
  for (const candidate of candidateAdbPaths()) {
    if (fs.existsSync(candidate)) {
      cachedAdb = candidate;
      return cachedAdb;
    }
  }
  const probe = spawnSync(adbName(), ['version'], { encoding: 'utf8' });
  if (probe.status === 0) {
    cachedAdb = adbName();
    return cachedAdb;
  }
  throw new Error('未找到 adb。请设置 ANDROID_HOME / ANDROID_SDK_ROOT，或将 adb 加入 PATH。');
}

function adbBaseArgs() {
  return process.env.ANDROID_SERIAL ? ['-s', process.env.ANDROID_SERIAL] : [];
}

function boundedAdbText(value) {
  if (value === undefined || value === null) return '';
  const text = Buffer.isBuffer(value) ? value.toString('utf8') : String(value);
  return text
    .replace(/[^\x20-\x7e\t\r\n]/g, '?')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, MAX_ADB_ERROR_TEXT);
}

function adbFailure(finalArgs, result, bytesWritten = 0) {
  const status = Number.isInteger(result?.status) ? result.status : 1;
  const stderr = boundedAdbText(result?.stderr);
  const signal = result?.signal ? `, signal=${result.signal}` : '';
  const error = new Error(
    `adb ${finalArgs.join(' ')} failed (status=${status}, bytes=${bytesWritten}${signal}, stderr=${stderr || 'unavailable'})`,
  );
  error.status = status;
  error.bytesWritten = Number.isSafeInteger(bytesWritten) && bytesWritten >= 0 ? bytesWritten : 0;
  error.stderr = stderr;
  error.stdoutOmitted = true;
  return error;
}

function runAdbResult(args, options = {}) {
  const adb = findAdb();
  const finalArgs = [...adbBaseArgs(), ...args];
  const result = spawnSync(adb, finalArgs, {
    encoding: options.encoding || 'utf8',
    stdio: options.stdio || 'pipe',
    maxBuffer: options.maxBuffer || 16 * 1024 * 1024,
  });
  return {
    status: Number.isInteger(result.status) ? result.status : 1,
    stdout: result.stdout || '',
    stderr: result.stderr || '',
  };
}

function runAdb(args, options = {}) {
  const finalArgs = [...adbBaseArgs(), ...args];
  const result = runAdbResult(args, options);
  if (result.status !== 0 && !options.allowFailure) {
    const stderr = result.stderr || '';
    const stdout = result.stdout || '';
    throw new Error(`adb ${finalArgs.join(' ')} failed\n${stdout}\n${stderr}`);
  }
  return result.stdout || '';
}

function runAdbBuffer(args, options = {}) {
  const adb = findAdb();
  const finalArgs = [...adbBaseArgs(), ...args];
  const result = spawnSync(adb, finalArgs, {
    encoding: null,
    stdio: 'pipe',
    maxBuffer: options.maxBuffer || 32 * 1024 * 1024,
  });
  if (result.status !== 0 && !options.allowFailure) {
    throw adbFailure(finalArgs, result, Buffer.isBuffer(result.stdout) ? result.stdout.length : 0);
  }
  return result.stdout || Buffer.alloc(0);
}

/**
 * Execute a binary adb command without buffering stdout in the Node process.
 * The child stdout is connected directly to a local file descriptor; only a
 * bounded stderr pipe is retained for diagnostics.
 */
function runAdbToFile(args, destination, options = {}) {
  const adb = findAdb();
  const finalArgs = [...adbBaseArgs(), ...args];
  let fd;
  let result;
  try {
    fd = fs.openSync(destination, 'w');
    result = spawnSync(adb, finalArgs, {
      encoding: 'utf8',
      stdio: ['ignore', fd, 'pipe'],
      maxBuffer: options.maxBuffer || 1024 * 1024,
    });
  } catch (error) {
    const bytesWritten = fs.statSync(destination, { throwIfNoEntry: false })?.size || 0;
    throw adbFailure(finalArgs, {
      status: Number.isInteger(error?.status) ? error.status : 1,
      stderr: error?.stderr || error?.message,
      signal: error?.signal,
    }, bytesWritten);
  } finally {
    if (fd !== undefined) {
      try {
        fs.closeSync(fd);
      } catch {}
    }
  }

  const bytesWritten = fs.statSync(destination, { throwIfNoEntry: false })?.size || 0;
  const normalized = {
    status: Number.isInteger(result?.status) ? result.status : 1,
    stderr: boundedAdbText(result?.stderr),
    signal: result?.signal,
    bytesWritten,
  };
  if (normalized.status !== 0 && !options.allowFailure) {
    throw adbFailure(finalArgs, normalized, bytesWritten);
  }
  return normalized;
}

function listDevices() {
  const adb = findAdb();
  const output = execFileSync(adb, ['devices'], { encoding: 'utf8' });
  return output
    .split(/\r?\n/)
    .slice(1)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [serial, state] = line.split(/\s+/);
      return { serial, state };
    });
}

function ensureSingleDevice() {
  const devices = listDevices().filter((device) => device.state === 'device');
  if (process.env.ANDROID_SERIAL) {
    const matched = devices.find((device) => device.serial === process.env.ANDROID_SERIAL);
    if (!matched) {
      throw new Error(`ANDROID_SERIAL=${process.env.ANDROID_SERIAL} 未连接或不在 device 状态。`);
    }
    return matched;
  }
  if (devices.length !== 1) {
    throw new Error(`需要且仅需要 1 台 device 状态设备；当前=${JSON.stringify(devices)}。多设备时请设置 ANDROID_SERIAL。`);
  }
  return devices[0];
}

function getProp(name) {
  return runAdb(['shell', 'getprop', name]).trim();
}

function getDeviceInfo() {
  const device = ensureSingleDevice();
  return {
    serial: device.serial,
    manufacturer: getProp('ro.product.manufacturer'),
    model: getProp('ro.product.model'),
    sdk: Number(getProp('ro.build.version.sdk') || '0'),
    release: getProp('ro.build.version.release'),
    abi: getProp('ro.product.cpu.abi'),
    packageDebug: DEBUG_PACKAGE,
  };
}

function timestamp() {
  return new Date().toISOString().replace(/[:.]/g, '-');
}

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

module.exports = {
  ROOT,
  DEBUG_PACKAGE,
  findAdb,
  runAdb,
  runAdbResult,
  runAdbBuffer,
  runAdbToFile,
  listDevices,
  ensureSingleDevice,
  getDeviceInfo,
  timestamp,
  ensureDir,
};

if (require.main === module) {
  if (process.argv.includes('--help') || process.argv.includes('-h')) {
    console.log(`Usage:
  node tests/e2e-android/scripts/adb-env.cjs

Prints adb path and the selected connected device as JSON.

Env:
  ANDROID_SERIAL  target device serial when multiple devices are connected
  ADB             explicit adb executable path
`);
    process.exit(0);
  }

  console.log(JSON.stringify({ adb: findAdb(), device: getDeviceInfo() }, null, 2));
}
