import { readdirSync, mkdirSync, copyFileSync, readFileSync, writeFileSync, mkdtempSync, renameSync, statSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { platforms, assetPath, assertVersion, assertSourceSha, signatureContent, publicKey, readJson, projectRoot, releaseSettings } from './release-updater.mjs';

export const sha256 = file => createHash('sha256').update(readFileSync(file)).digest('hex');
function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap(e => e.isDirectory() ? walk(path.join(dir, e.name)) : e.isFile() ? [path.join(dir, e.name)] : []);
}
function exactlyOne(files, suffix, label) {
  const matches = files.filter(file => file.endsWith(suffix));
  if (matches.length !== 1) throw new Error(`Expected exactly one ${label} ${suffix} artifact, found ${matches.length}`);
  if (!statSync(matches[0]).size) throw new Error(`Empty ${label} ${suffix} artifact`);
  return matches[0];
}
export function collectRelease({ label, bundleDirectory, outputDirectory, version, sourceSha, updaterPublicKey, portable = true, runtime = process }) {
  const platform = platforms[label];
  if (!platform) throw new Error('Invalid platform label');
  if (runtime.platform !== platform.os || runtime.arch !== platform.arch) throw new Error(`Runner architecture does not match ${label}`);
  assertVersion(version); assertSourceSha(sourceSha);
  const files = walk(bundleDirectory);
  const installers = platform.installers.map(suffix => exactlyOne(files, suffix, label));
  const updater = exactlyOne(files, platform.updater, label);
  const updaterFiles = [updater, ...(platform.additionalUpdaters ?? []).map(extra => exactlyOne(files, extra.suffix, label))];
  const signatures = updaterFiles.map(file => `${file}.sig`);
  // Never publish an unsigned update or silently switch to the ordinary installer.
  for (const signature of signatures) signatureContent(readFileSync(signature, 'utf8'), version, updaterPublicKey);
  mkdirSync(outputDirectory, { recursive: true });
  const checksums = [], names = new Map();
  for (const file of new Set([...installers, ...updaterFiles, ...signatures])) {
    const name = `${label}-${path.basename(file).replaceAll(' ', '-')}`;
    copyFileSync(file, assetPath(outputDirectory, name));
    names.set(file, name);
    checksums.push(`${sha256(file)}  ${name}`);
  }
  const extraUpdaters = Object.fromEntries((platform.additionalUpdaters ?? []).map((extra, index) => {
    const file = updaterFiles[index + 1];
    return [extra.target, { file: names.get(file), signatureFile: names.get(`${file}.sig`), sha256: sha256(file) }];
  }));
  const metadata = { schemaVersion: 1, label, target: platform.target, version, sourceSha, file: names.get(updater), signatureFile: names.get(`${updater}.sig`), sha256: sha256(updater), extraUpdaters };
  const metadataName = `updater-${label}.json`;
  writeFileSync(assetPath(outputDirectory, metadataName), JSON.stringify(metadata, null, 2) + '\n');
  checksums.push(`${sha256(assetPath(outputDirectory, metadataName))}  ${metadataName}`);
  // A pre-extracted distribution works without FUSE, apt, root, or runtime flags.
  if (label === 'linux-x64' && portable) {
    const staging = mkdtempSync(path.resolve(bundleDirectory, '../portable-'));
    execFileSync(path.resolve(updater), ['--appimage-extract'], { cwd: staging, stdio: 'ignore' });
    renameSync(path.join(staging, 'squashfs-root'), path.join(staging, 'HTTP-Capture'));
    writeFileSync(path.join(staging, 'HTTP-Capture', 'START-HERE.txt'),
      'Extract this entire folder, then run AppRun. Keep all bundled files together.\n' +
      'No apt, root, FUSE, or graphics environment settings are required for this launcher.\n' +
      'Linux x64, Ubuntu 22.04 or compatible newer glibc desktop required.\n' +
      '解压整个目录，运行 AppRun；请保留目录内所有文件。无需安装 FUSE 或手动配置渲染环境变量。\n');
    const name = `linux-x64-${path.basename(updater, '.AppImage').replaceAll(' ', '-')}-portable.tar.gz`;
    const archive = path.resolve(assetPath(outputDirectory, name));
    execFileSync('tar', ['-czf', archive, '-C', staging, 'HTTP-Capture'], { stdio: 'inherit' });
    checksums.push(`${sha256(archive)}  ${name}`);
  }
  writeFileSync(assetPath(outputDirectory, `SHA256SUMS-${label}.txt`), checksums.join('\n') + '\n');
  return metadata;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const config = readJson(path.join(projectRoot, 'apps/desktop/src-tauri/tauri.conf.json'));
  const settings = releaseSettings(config, process.env);
  const built = readJson(path.join(projectRoot, 'apps/desktop/src-tauri/tauri.release.json'));
  if (built.version !== settings.version || built.bundle?.createUpdaterArtifacts !== true) throw new Error('Prepared Tauri updater configuration does not match this release');
  collectRelease({ label: process.argv[2], bundleDirectory: path.join(projectRoot, 'target/release/bundle'), outputDirectory: path.join(projectRoot, 'release-assets'), updaterPublicKey: publicKey(config), ...settings });
}
