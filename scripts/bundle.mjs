#!/usr/bin/env node
// Packages AgoraLume into a single distributable zip.
//
// Output: dist-bundle/AgoraLume-<platform>-<arch>.zip, containing an
// `AgoraLume/` folder with the backend executable and the built frontend in
// `web/`. Unzip it and run the executable — it serves the API and the SPA from
// one origin and opens a browser. See resolve_web_dir() in backend/src/main.rs
// for how the executable finds `web/`.
//
// Run from anywhere: `node scripts/bundle.mjs`.

import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '..');
const isWindows = process.platform === 'win32';
const exeName = isWindows ? 'agoralume-backend.exe' : 'agoralume-backend';

// A step that must succeed; inherits stdio so build output streams live.
function run(cmd, args, cwd = repoRoot) {
  console.log(`\n$ ${cmd} ${args.join(' ')}`);
  const res = spawnSync(cmd, args, { cwd, stdio: 'inherit', shell: isWindows });
  if (res.status !== 0) {
    throw new Error(`${cmd} exited with ${res.status ?? res.signal}`);
  }
}

// 1. Build the frontend in same-origin mode (defaults to the serving origin,
//    not the in-browser mock).
run('pnpm', ['build:bundle'], join(repoRoot, 'frontend'));

// 2. Build the backend as an optimized single executable.
run('cargo', ['build', '--release', '--manifest-path', join(repoRoot, 'backend', 'Cargo.toml')]);

// 3. Stage `AgoraLume/` with the executable beside `web/`.
const outDir = join(repoRoot, 'dist-bundle');
const stageDir = join(outDir, 'AgoraLume');
rmSync(stageDir, { recursive: true, force: true });
mkdirSync(stageDir, { recursive: true });
cpSync(join(repoRoot, 'backend', 'target', 'release', exeName), join(stageDir, exeName));
cpSync(join(repoRoot, 'frontend', 'dist'), join(stageDir, 'web'), { recursive: true });
// Ship the settings template beside the exe; users copy it to `.env` (which the
// backend loads on startup) to configure without exporting env vars by hand.
cpSync(join(repoRoot, '.env.example'), join(stageDir, '.env.example'));

// 4. Zip the staged folder with each OS's standard tool.
const zipName = `AgoraLume-${process.platform}-${process.arch}.zip`;
const zipPath = join(outDir, zipName);
rmSync(zipPath, { force: true });
if (isWindows) {
  run('powershell', [
    '-NoProfile',
    '-Command',
    `Compress-Archive -Path '${stageDir}' -DestinationPath '${zipPath}' -Force`,
  ]);
} else {
  // `zip` keeps the AgoraLume/ prefix when run from the parent directory.
  run('zip', ['-r', zipName, 'AgoraLume'], outDir);
}

console.log(`\n✓ Bundle ready: ${zipPath}`);
console.log('  Unzip it and run the executable inside the AgoraLume/ folder.');
