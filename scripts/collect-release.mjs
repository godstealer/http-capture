import { readdirSync, mkdirSync, copyFileSync, readFileSync, writeFileSync, mkdtempSync, renameSync } from 'node:fs';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

const label = process.argv[2];
if (!/^(windows-x64|linux-x64|macos-arm64|macos-x64)$/.test(label)) throw new Error('Invalid platform label');
const expected = { 'windows-x64': ['win32', 'x64'], 'linux-x64': ['linux', 'x64'], 'macos-arm64': ['darwin', 'arm64'], 'macos-x64': ['darwin', 'x64'] }[label];
if (process.platform !== expected[0] || process.arch !== expected[1]) throw new Error(`Runner architecture does not match ${label}`);
function walk(dir) { return readdirSync(dir, { withFileTypes: true }).flatMap(e => e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)]); }
const suffixes = { 'windows-x64': ['.exe'], 'linux-x64': ['.deb', '.AppImage'], 'macos-arm64': ['.dmg'], 'macos-x64': ['.dmg'] }[label];
const files = walk('target/release/bundle').filter(f => suffixes.some(suffix => f.endsWith(suffix)));
for (const suffix of suffixes) if (!files.some(f => f.endsWith(suffix))) throw new Error(`Missing ${label} ${suffix} installer`);
mkdirSync('release-assets', { recursive: true });
const checksums = [];
for (const file of files) {
  const name = `${label}-${path.basename(file).replaceAll(' ', '-')}`;
  copyFileSync(file, path.join('release-assets', name));
  checksums.push(`${createHash('sha256').update(readFileSync(file)).digest('hex')}  ${name}`);
}
// A pre-extracted distribution works without FUSE, apt, root, or runtime flags.
if (label === 'linux-x64') {
  const appImages = files.filter(file => file.endsWith('.AppImage'));
  if (appImages.length !== 1) throw new Error('Expected exactly one AppImage for the portable archive');
  const staging = mkdtempSync(path.resolve('target/release/portable-'));
  execFileSync(path.resolve(appImages[0]), ['--appimage-extract'], { cwd: staging, stdio: 'ignore' });
  renameSync(path.join(staging, 'squashfs-root'), path.join(staging, 'HTTP-Capture'));
  writeFileSync(path.join(staging, 'HTTP-Capture', 'START-HERE.txt'),
    'Extract this entire folder, then run AppRun. Keep all bundled files together.\n' +
    'No apt, root, FUSE, or graphics environment settings are required for this launcher.\n' +
    'Linux x64, Ubuntu 22.04 or compatible newer glibc desktop required.\n' +
    '解压整个目录，运行 AppRun；请保留目录内所有文件。无需安装 FUSE 或手动配置渲染环境变量。\n');
  const name = `linux-x64-${path.basename(appImages[0], '.AppImage').replaceAll(' ', '-')}-portable.tar.gz`;
  const archive = path.resolve('release-assets', name);
  execFileSync('tar', ['-czf', archive, '-C', staging, 'HTTP-Capture'], { stdio: 'inherit' });
  checksums.push(`${createHash('sha256').update(readFileSync(archive)).digest('hex')}  ${name}`);
}
writeFileSync(`release-assets/SHA256SUMS-${label}.txt`, checksums.join('\n') + '\n');
