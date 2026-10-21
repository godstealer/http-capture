import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

export const platforms = Object.freeze({
  'windows-x64': { target: 'windows-x86_64', os: 'win32', arch: 'x64', installers: ['.exe'], updater: '.exe' },
  'linux-x64': { target: 'linux-x86_64', os: 'linux', arch: 'x64', installers: ['.deb', '.AppImage'], updater: '.AppImage', additionalUpdaters: [{ target: 'linux-x86_64-deb', suffix: '.deb' }] },
  'macos-arm64': { target: 'darwin-aarch64', os: 'darwin', arch: 'arm64', installers: ['.dmg'], updater: '.app.tar.gz' },
  'macos-x64': { target: 'darwin-x86_64', os: 'darwin', arch: 'x64', installers: ['.dmg'], updater: '.app.tar.gz' },
});
export const updaterTargets = Object.entries(platforms).flatMap(([label, platform]) => [
  { label, target: platform.target, suffix: platform.updater },
  ...(platform.additionalUpdaters ?? []).map(extra => ({ label, ...extra })),
]);
export const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const releaseNotesPath = 'docs/release-2026-10-21-certificates.md';
export function readReleaseNotes(rootDirectory = projectRoot) {
  const markdown = readFileSync(path.join(rootDirectory, releaseNotesPath), 'utf8').replace(/^\uFEFF/, '').trim();
  const summary = markdown.split(/^## 更新内容\r?$/m)[1]?.split(/^## /m)[0].trim();
  if (!summary) throw new Error('Release notes must contain a non-empty 更新内容 section');
  return { markdown, summary };
}
export function readJson(file) { return JSON.parse(readFileSync(file, 'utf8').replace(/^\uFEFF/, '')); }
export function assertVersion(value) {
  if (typeof value !== 'string' || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(value)) throw new Error('Release version must be a numeric major.minor.patch SemVer');
  return value;
}
export function releaseVersion(base, checkRunNumber) {
  assertVersion(base);
  const number = String(checkRunNumber ?? '');
  if (!/^[1-9]\d*$/.test(number) || !Number.isSafeInteger(Number(number))) throw new Error('SOURCE_CHECK_RUN_NUMBER must be the positive source Check run_number');
  return `${base.split('.').slice(0, 2).join('.')}.${number}`;
}
export function assertSourceSha(sha) {
  if (typeof sha !== 'string' || !/^[a-f0-9]{40}$/.test(sha)) throw new Error('SOURCE_SHA must be a full commit SHA');
  return sha;
}
export function buildTag(sha) { return `build-${assertSourceSha(sha).slice(0, 12)}`; }
export function assertRepository(repo) {
  if (typeof repo !== 'string' || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo)) throw new Error('Invalid GitHub repository');
  return repo;
}
export function assetPath(directory, name) {
  if (typeof name !== 'string' || !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name) || name.includes('..')) throw new Error('Unsafe release asset filename');
  return path.join(directory, name);
}
function canonicalBase64(value, message) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) throw new Error(message);
  const decoded = Buffer.from(value, 'base64');
  // Node ignores invalid characters, missing padding and non-zero padding bits.
  // The desktop verifier is strict, so every layer must have one canonical form.
  if (decoded.toString('base64') !== value) throw new Error(message);
  return decoded;
}
function envelopeLines(bytes, message) {
  const text = bytes.toString('utf8');
  if (!Buffer.from(text, 'utf8').equals(bytes)) throw new Error(message);
  // Only remove the final line ending. Trimming the envelope would hide illegal
  // whitespace appended to the final Base64 line from strict validation.
  return text.replace(/\r?\n$/, '').split(/\r?\n/);
}
export function signatureContent(text, expectedVersion, updaterPublicKey) {
  const value = typeof text === 'string' ? text.trim() : '';
  const invalid = 'Invalid Tauri updater signature envelope: expected canonical base64';
  const decoded = envelopeLines(canonicalBase64(value, 'Updater signature must contain canonical base64 content of a .sig file'), invalid);
  if (decoded.length !== 4 || !decoded[0].startsWith('untrusted comment: ') || !decoded[2].startsWith('trusted comment: ')) throw new Error('Invalid Tauri updater signature envelope');
  const payload = canonicalBase64(decoded[1], invalid);
  if (payload.length !== 74 || canonicalBase64(decoded[3], invalid).length !== 64) throw new Error('Invalid Tauri updater signature envelope');
  // Match the desktop updater's tab-separated strip_prefix("version:") parser.
  // Whitespace is signed data, not normalization: accepting it here would claim
  // a package is installable when the application rejects its signed version.
  const versions = decoded[2].slice('trusted comment: '.length).split('\t').filter(field => field.startsWith('version:'));
  if (expectedVersion && (versions.length !== 1 || versions[0].slice('version:'.length) !== assertVersion(expectedVersion))) throw new Error('Updater signature is missing the matching signed app version; use Tauri CLI 2.11.5 or newer');
  if (updaterPublicKey) {
    const trustedKey = publicKey({ plugins: { updater: { pubkey: updaterPublicKey } } });
    const key = canonicalBase64(envelopeLines(canonicalBase64(trustedKey, invalid), invalid)[1], invalid);
    if (key.length !== 42 || !key.subarray(2, 10).equals(payload.subarray(2, 10))) throw new Error('Updater signature key id differs from the committed public key');
  }
  return value;
}
export function publicKey(config, provided) {
  const key = config.plugins?.updater?.pubkey?.trim();
  if (!key) throw new Error('Missing committed plugins.updater.pubkey in tauri.conf.json');
  const invalid = 'Invalid committed Tauri updater public key: expected canonical base64';
  const decoded = envelopeLines(canonicalBase64(key, invalid), invalid);
  if (decoded.length !== 2 || !decoded[0].startsWith('untrusted comment: ') || canonicalBase64(decoded[1], invalid).length !== 42) throw new Error('Invalid committed Tauri updater public key');
  if (provided?.trim() && key !== provided.trim()) throw new Error('TAURI_UPDATER_PUBLIC_KEY differs from the committed updater public key');
  return key;
}
export function releaseSettings(config, env) {
  const version = releaseVersion(config.version, env.SOURCE_CHECK_RUN_NUMBER);
  if (env.RELEASE_VERSION && env.RELEASE_VERSION !== version) throw new Error('RELEASE_VERSION differs from the source Check version');
  assertSourceSha(env.SOURCE_SHA);
  publicKey(config, env.TAURI_UPDATER_PUBLIC_KEY);
  return { version, sourceSha: env.SOURCE_SHA };
}
export function releaseConfiguration(config, env) {
  const { version } = releaseSettings(config, env);
  return {
    version,
    bundle: {
      createUpdaterArtifacts: true,
      resources: { 'release-resources/': 'helpers/' },
      icon: ['icons/32x32.png', 'icons/128x128.png', 'icons/128x128@2x.png', 'icons/icon.icns', 'icons/icon.ico'],
    },
  };
}
export function checkSigningSecrets(env) {
  if (!env.TAURI_SIGNING_PRIVATE_KEY?.trim()) throw new Error('Missing GitHub Secret TAURI_SIGNING_PRIVATE_KEY; signed updater artifacts are required');
  if (!env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD?.trim()) throw new Error('Missing GitHub Secret TAURI_SIGNING_PRIVATE_KEY_PASSWORD; use the password for the encrypted updater signing key');
}
export function compareVersions(a, b) {
  const left = assertVersion(a).split('.').map(BigInt), right = assertVersion(b).split('.').map(BigInt);
  for (let i = 0; i < 3; i++) if (left[i] !== right[i]) return left[i] > right[i] ? 1 : -1;
  return 0;
}
export function shouldPromote(candidate, current) {
  assertVersion(candidate.version); assertSourceSha(candidate.source_sha);
  if (!current) return true;
  const comparison = compareVersions(candidate.version, current.version);
  if (comparison < 0) return false;
  if (comparison > 0) return true;
  if (candidate.source_sha !== current.source_sha || JSON.stringify(candidate) !== JSON.stringify(current)) throw new Error('Refusing to replace an existing updater version with different source or artifacts');
  return false;
}
export function validateManifest(manifest, repository) {
  assertVersion(manifest.version); assertSourceSha(manifest.source_sha); assertRepository(repository);
  if (JSON.stringify(Object.keys(manifest.platforms ?? {}).sort()) !== JSON.stringify(updaterTargets.map(p => p.target).sort())) throw new Error('Updater manifest must contain all four desktop platforms, including the Debian updater target');
  for (const { label, target, suffix } of updaterTargets) {
    const entry = manifest.platforms[target];
    signatureContent(entry.signature, manifest.version);
    const url = new URL(entry.url);
    const prefix = `https://github.com/${repository}/releases/download/${buildTag(manifest.source_sha)}/`;
    if (!entry.url.startsWith(prefix) || url.origin !== 'https://github.com' || url.search || url.hash) throw new Error('Updater URLs must use the immutable source build tag');
    const name = decodeURIComponent(entry.url.slice(prefix.length));
    assetPath('.', name);
    if (!name.startsWith(`${label}-`) || !name.endsWith(suffix)) throw new Error('Updater URL does not match its platform');
  }
  return manifest;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const config = readJson(path.join(projectRoot, 'apps/desktop/src-tauri/tauri.conf.json'));
  const settings = releaseSettings(config, process.env);
  if (process.argv[2] === 'preflight') {
    checkSigningSecrets(process.env);
    console.log(`Updater signing prerequisites validated; release version ${settings.version}`);
  } else if (process.argv[2] === 'version') console.log(settings.version);
  else throw new Error('Expected preflight or version');
}
