import { readdirSync, mkdirSync, copyFileSync, readFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import path from 'node:path';

const label = process.argv[2];
if (!/^(windows-x64|linux-x64|macos-arm64|macos-x64)$/.test(label)) throw new Error('Invalid platform label');
const expected = { 'windows-x64': ['win32', 'x64'], 'linux-x64': ['linux', 'x64'], 'macos-arm64': ['darwin', 'arm64'], 'macos-x64': ['darwin', 'x64'] }[label];
if (process.platform !== expected[0] || process.arch !== expected[1]) throw new Error(`Runner architecture does not match ${label}`);
function walk(dir) { return readdirSync(dir, { withFileTypes: true }).flatMap(e => e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)]); }
const suffix = { 'windows-x64': '.exe', 'linux-x64': '.deb', 'macos-arm64': '.dmg', 'macos-x64': '.dmg' }[label];
const files = walk('target/release/bundle').filter(f => f.endsWith(suffix));
if (!files.length) throw new Error(`Missing ${label} installer`);
mkdirSync('release-assets', { recursive: true });
const checksums = [];
for (const file of files) {
  const name = `${label}-${path.basename(file).replaceAll(' ', '-')}`;
  copyFileSync(file, path.join('release-assets', name));
  checksums.push(`${createHash('sha256').update(readFileSync(file)).digest('hex')}  ${name}`);
}
writeFileSync(`release-assets/SHA256SUMS-${label}.txt`, checksums.join('\n') + '\n');
