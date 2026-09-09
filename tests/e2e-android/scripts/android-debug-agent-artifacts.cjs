'use strict';

const fs = require('fs');
const path = require('path');
const {
  DEBUG_PACKAGE,
  ROOT,
  runAdbBuffer,
  timestamp,
  ensureDir,
} = require('./adb-env.cjs');
const {
  printPayload,
  resolveOutputPath,
} = require('./android-debug-agent-args.cjs');
const { collectLogs, collectStatus, ensureUsbDevice } = require('./android-debug-agent-status.cjs');

function captureScreenshot(outputPath) {
  const png = runAdbBuffer(['exec-out', 'screencap', '-p']);
  const pngSignature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  if (png.length < pngSignature.length || !png.subarray(0, pngSignature.length).equals(pngSignature)) {
    throw new Error('adb screencap did not return a PNG payload');
  }
  ensureDir(path.dirname(outputPath));
  fs.writeFileSync(outputPath, png);
  return { path: outputPath, bytes: png.length };
}

function screenshotPath(options, config) {
  if (options.out) return resolveOutputPath(options.out, '');
  const slug = (options.name || 'screen').replace(/[^a-zA-Z0-9_-]/g, '-').slice(0, 48) || 'screen';
  return path.join(config.ARTIFACT_ROOT, 'screenshots', `${timestamp()}-${slug}.png`);
}

async function commandScreenshot(options, config) {
  ensureUsbDevice();
  const result = captureScreenshot(screenshotPath(options, config));
  printPayload(options, {
    schema: `vcp.android-debug.screenshot.v${config.SCHEMA_VERSION}`,
    package: DEBUG_PACKAGE,
    ...result,
    imageEmbeddedInStdout: false,
  });
  return 0;
}

async function commandSnapshot(options, config) {
  ensureUsbDevice();
  const status = await collectStatus(config);
  const logs = collectLogs(options, status, config);
  const outDir = resolveOutputPath(
    options.out,
    path.join('.agent', 'android-debug', 'snapshots', timestamp()),
  );
  ensureDir(outDir);
  fs.writeFileSync(path.join(outDir, 'status.json'), `${JSON.stringify(status, null, 2)}\n`, 'utf8');
  fs.writeFileSync(path.join(outDir, 'logcat.txt'), `${logs.lines.join('\n')}\n`, 'utf8');
  const screenshot = options.screenshot ? captureScreenshot(path.join(outDir, 'screen.png')) : null;
  const manifest = {
    schema: `vcp.android-debug.snapshot.v${config.SCHEMA_VERSION}`,
    generatedAt: new Date().toISOString(),
    package: DEBUG_PACKAGE,
    files: {
      status: path.join(outDir, 'status.json'),
      logs: path.join(outDir, 'logcat.txt'),
      screenshot: screenshot?.path || null,
    },
    logLines: logs.lines.length,
    screenshotBytes: screenshot?.bytes || 0,
  };
  fs.writeFileSync(path.join(outDir, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
  printPayload(options, manifest);
  return 0;
}

module.exports = {
  captureScreenshot,
  commandScreenshot,
  commandSnapshot,
};
