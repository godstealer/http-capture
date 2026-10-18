import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync, unlinkSync, rmSync } from 'node:fs';
import path from 'node:path';
import { platforms, projectRoot, releaseVersion, releaseSettings, releaseConfiguration, publicKey, checkSigningSecrets, signatureContent, shouldPromote, validateManifest, assetPath, readReleaseNotes, releaseNotesPath } from './release-updater.mjs';
import { collectRelease } from './collect-release.mjs';
import { generateUpdater } from './generate-updater.mjs';
import { publishRelease } from './publish-release.mjs';

const version = '0.1.42', sourceSha = 'a'.repeat(40), repository = 'godstealer/http-capture', publishedAt = '2026-10-16T01:02:03Z';
const publicBytes = Buffer.alloc(42, 1); publicBytes.write('Ed');
const key = Buffer.from(`untrusted comment: fixture public key\n${publicBytes.toString('base64')}\n`).toString('base64');
const config = { version: '0.1.0', plugins: { updater: { pubkey: key, requireSignedVersion: true, endpoints: ['https://github.com/godstealer/http-capture/releases/download/updater-preview/latest.json'] } } };
const env = { SOURCE_CHECK_RUN_NUMBER: '42', SOURCE_SHA: sourceSha };
function signature(signedVersion = version, keyId = 1) {
  const payload = Buffer.alloc(74, 2); payload.write('ED'); payload.fill(keyId, 2, 10);
  return Buffer.from(`untrusted comment: fixture signature\n${payload.toString('base64')}\ntrusted comment: timestamp:1\t file:fixture${signedVersion == null ? '' : `\t version:${signedVersion}`}\n${Buffer.alloc(64, 3).toString('base64')}\n`).toString('base64');
}
function fixture(t) {
  const parent = path.join(projectRoot, '.local'); mkdirSync(parent, { recursive: true });
  const directory = mkdtempSync(path.join(parent, 'release-updater-test-'));
  t.after(() => {
    // Only remove the exact fresh fixture directory under this workspace.
    assert.equal(path.dirname(path.resolve(directory)), path.resolve(parent));
    assert(path.basename(directory).startsWith('release-updater-test-'));
    rmSync(directory, { recursive: true, force: true });
  });
  const outputDirectory = path.join(directory, 'release-assets');
  return { directory, outputDirectory };
}
function buildFixture(t) {
  const f = fixture(t); const metadata = {};
  for (const [label, p] of Object.entries(platforms)) {
    const bundleDirectory = path.join(f.directory, label); mkdirSync(bundleDirectory);
    for (const suffix of new Set([...p.installers, p.updater])) writeFileSync(path.join(bundleDirectory, `HTTP Capture_${version}${suffix}`), `${label}:${suffix}`);
    for (const suffix of [p.updater, ...(p.additionalUpdaters ?? []).map(extra => extra.suffix)]) writeFileSync(path.join(bundleDirectory, `HTTP Capture_${version}${suffix}.sig`), signature() + '\n');
    metadata[label] = collectRelease({ label, bundleDirectory, outputDirectory: f.outputDirectory, version, sourceSha, updaterPublicKey: key, runtime: { platform: p.os, arch: p.arch }, portable: false });
  }
  return { ...f, metadata, generate: (overrides = {}) => generateUpdater({ directory: f.outputDirectory, version, sourceSha, repository, publishedAt, updaterPublicKey: key, ...overrides }) };
}
function saveMetadata(f, label, changes) {
  writeFileSync(path.join(f.outputDirectory, `updater-${label}.json`), JSON.stringify({ ...f.metadata[label], ...changes }));
}

test('versions use source Check numbers, stay stable on retries, and inherit only major/minor', () => {
  assert.equal(releaseVersion('0.1.0', 42), version);
  assert.equal(releaseVersion('2.7.9', 42), '2.7.42');
  assert.deepEqual(releaseSettings(config, { ...env, GITHUB_RUN_NUMBER: '500', GITHUB_RUN_ATTEMPT: '2' }), { version, sourceSha });
  assert.throws(() => releaseSettings(config, { ...env, RELEASE_VERSION: '0.1.41' }), /differs/);
  for (const invalid of [undefined, '', '0', '-1', '1.5', '01', 'x']) assert.throws(() => releaseVersion('0.1.0', invalid));
  assert.throws(() => releaseVersion('0.1.0-beta', 42), /SemVer/);
});
test('release overlay enables updater artifacts without erasing the committed trust configuration', () => {
  const overlay = releaseConfiguration(config, env);
  assert.equal(overlay.version, version); assert.equal(overlay.bundle.createUpdaterArtifacts, true);
  assert.equal(overlay.plugins, undefined);
  assert.equal({ ...config, ...overlay }.plugins.updater.pubkey, key);
  assert.equal({ ...config, ...overlay }.plugins.updater.requireSignedVersion, true);
});
test('missing signing secrets or trust key fail clearly; public key variable is optional and must agree', () => {
  assert.equal(publicKey(config), key); assert.equal(publicKey(config, key), key);
  assert.throws(() => publicKey(config, 'different'), /differs/);
  assert.throws(() => publicKey({}), /Missing committed/);
  assert.throws(() => checkSigningSecrets({}), /TAURI_SIGNING_PRIVATE_KEY/);
  assert.throws(() => checkSigningSecrets({ TAURI_SIGNING_PRIVATE_KEY: 'fixture' }), /PRIVATE_KEY_PASSWORD/);
  checkSigningSecrets({ TAURI_SIGNING_PRIVATE_KEY: 'fixture', TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'fixture' });
});
test('four platform fixtures produce distinct immutable URLs, .sig contents and checksums', t => {
  const f = buildFixture(t), manifest = f.generate();
  assert.deepEqual(Object.keys(manifest.platforms).sort(), ['darwin-aarch64', 'darwin-x86_64', 'linux-x86_64', 'linux-x86_64-deb', 'windows-x86_64']);
  assert.equal(manifest.version, version); assert.equal(manifest.pub_date, publishedAt);
  assert.equal(manifest.source_sha, sourceSha);
  for (const [label, p] of Object.entries(platforms)) {
    const entry = manifest.platforms[p.target], meta = f.metadata[label];
    assert.equal(entry.signature, signature());
    assert.equal(entry.url, `https://github.com/${repository}/releases/download/build-${sourceSha.slice(0, 12)}/${meta.file}`);
    assert.equal(readFileSync(path.join(f.outputDirectory, meta.signatureFile), 'utf8').trim(), entry.signature);
    const sums = readFileSync(path.join(f.outputDirectory, `SHA256SUMS-${label}.txt`), 'utf8');
    assert(sums.includes(`${meta.sha256}  ${meta.file}`)); assert(sums.includes(meta.signatureFile));
  }
  assert.notEqual(f.metadata['macos-x64'].file, f.metadata['macos-arm64'].file);
  assert(manifest.platforms['linux-x86_64-deb'].url.endsWith('.deb'));
  assert(manifest.platforms['linux-x86_64'].url.endsWith('.AppImage'));
});
test('Debian updater requires its own signed package and cannot fall back to AppImage', t => {
  const f = buildFixture(t), deb = f.metadata['linux-x64'].extraUpdaters['linux-x86_64-deb'];
  unlinkSync(path.join(f.outputDirectory, deb.signatureFile));
  assert.throws(() => f.generate(), /ENOENT/);
});
test('collector rejects missing signatures, duplicate artifacts and wrong architecture', t => {
  const f = fixture(t), bundleDirectory = path.join(f.directory, 'bundle'); mkdirSync(bundleDirectory);
  const options = { label: 'windows-x64', bundleDirectory, outputDirectory: f.outputDirectory, version, sourceSha, runtime: { platform: 'win32', arch: 'x64' } };
  writeFileSync(path.join(bundleDirectory, 'app.exe'), 'installer');
  assert.throws(() => collectRelease(options), /ENOENT/);
  writeFileSync(path.join(bundleDirectory, 'app.exe.sig'), signature());
  assert.throws(() => collectRelease({ ...options, runtime: { platform: 'darwin', arch: 'x64' } }), /architecture/);
  writeFileSync(path.join(bundleDirectory, 'second.exe'), 'installer');
  assert.throws(() => collectRelease(options), /exactly one/);
});
test('manifest rejects incomplete platforms, missing .sig files and changed package bytes', t => {
  const f = buildFixture(t), meta = f.metadata['windows-x64'];
  const sig = path.join(f.outputDirectory, meta.signatureFile); unlinkSync(sig);
  assert.throws(() => f.generate(), /ENOENT/); writeFileSync(sig, signature());
  writeFileSync(path.join(f.outputDirectory, meta.file), 'changed'); assert.throws(() => f.generate(), /checksum/);
  unlinkSync(path.join(f.outputDirectory, 'updater-macos-arm64.json')); assert.throws(() => f.generate(), /ENOENT|checksum/);
});
test('manifest requires a complete four-platform matrix even if the Windows package is present', t => {
  const f = buildFixture(t); unlinkSync(path.join(f.outputDirectory, 'updater-macos-arm64.json'));
  assert.throws(() => f.generate(), /ENOENT/);
});
test('mismatched source/version, unsafe asset paths and unstable download URLs fail closed', t => {
  const f = buildFixture(t);
  saveMetadata(f, 'windows-x64', { version: '0.1.41' }); assert.throws(() => f.generate(), /identity/);
  saveMetadata(f, 'windows-x64', { sourceSha: 'b'.repeat(40) }); assert.throws(() => f.generate(), /identity/);
  saveMetadata(f, 'windows-x64', { file: '../outside.exe' }); assert.throws(() => f.generate(), /Unsafe/);
  saveMetadata(f, 'windows-x64', {});
  const manifest = f.generate(); manifest.platforms['windows-x86_64'].url = 'https://github.com/godstealer/http-capture/releases/latest/download/app.exe';
  assert.throws(() => validateManifest(manifest, repository), /immutable/);
  delete manifest.platforms['darwin-aarch64']; assert.throws(() => validateManifest(manifest, repository), /all four/);
  for (const name of ['../outside.exe', 'C:\\outside.exe', '/absolute.exe', 'sub/file.exe', 'sub\\file.exe']) assert.throws(() => assetPath(f.directory, name), /Unsafe/);
});
test('signed version and signing key id must match the release trust configuration', () => {
  assert.equal(signatureContent(signature(), version, key), signature());
  assert.throws(() => signatureContent(signature(null), version, key), /signed app version/);
  assert.throws(() => signatureContent(signature('0.1.41'), version, key), /signed app version/);
  assert.throws(() => signatureContent(signature(version, 9), version, key), /key id/);
  assert.throws(() => signatureContent('https://example.com/app.sig', version), /base64/);
});
test('delayed old builds and reruns cannot roll the preview channel back or replace a version', t => {
  const f = buildFixture(t), candidate = f.generate();
  assert(shouldPromote(candidate, null));
  assert(shouldPromote(candidate, { ...candidate, version: '0.1.9' }));
  assert.equal(shouldPromote(candidate, { ...candidate, version: '0.1.100' }), false);
  assert.equal(shouldPromote(candidate, JSON.parse(JSON.stringify(candidate))), false);
  assert.throws(() => shouldPromote(candidate, { ...candidate, source_sha: 'b'.repeat(40) }), /different/);
  assert.throws(() => shouldPromote(candidate, { ...candidate, notes: 'different bytes' }), /different/);
});

function publicationFixture(t, initial = {}, initialLatestTag = null) {
  const f = buildFixture(t), manifest = f.generate();
  writeFileSync(path.join(f.outputDirectory, 'latest.json'), JSON.stringify(manifest));
  const configDirectory = path.join(f.directory, 'apps/desktop/src-tauri'); mkdirSync(configDirectory, { recursive: true });
  writeFileSync(path.join(configDirectory, 'tauri.conf.json'), JSON.stringify(config));
  mkdirSync(path.join(f.directory, 'docs'));
  writeFileSync(path.join(f.directory, releaseNotesPath), readFileSync(path.join(projectRoot, releaseNotesPath)));
  const temporaryDirectory = path.join(f.directory, 'publish'); mkdirSync(temporaryDirectory);
  const releases = structuredClone(initial), calls = [];
  let latestTag = initialLatestTag;
  const run = args => {
    calls.push(args);
    if (args[0] === 'api') {
      const tag = args[1].endsWith('/latest') ? latestTag : args[1].split('/').at(-1);
      if (releases[tag]) return JSON.stringify({ tag_name: tag, assets: releases[tag].manifest ? [{ name: 'latest.json' }] : [], ...releases[tag] });
      throw Object.assign(new Error('fixture not found'), { stderr: 'gh: Not Found (HTTP 404)' });
    }
    const [_, action, tag] = args;
    if (action === 'download') writeFileSync(args[args.indexOf('--output') + 1], JSON.stringify(releases[tag].manifest));
    else if (action === 'create') releases[tag] = { draft: true, name: args[args.indexOf('--title') + 1], prerelease: args.includes('--prerelease') };
    else if (action === 'upload') {
      const file = args.find(value => path.basename(value) === 'latest.json');
      releases[tag].manifest = JSON.parse(readFileSync(file, 'utf8'));
    } else if (action === 'edit') {
      if (args.includes('--draft=false')) releases[tag].draft = false;
      if (args.includes('--prerelease=false')) releases[tag].prerelease = false;
      else if (args.includes('--prerelease')) releases[tag].prerelease = true;
      if (args.includes('--title')) releases[tag].name = args[args.indexOf('--title') + 1];
      if (args.includes('--latest=true')) latestTag = tag;
    }
    else throw new Error(`Unexpected fixture gh action: ${action}`);
    return '';
  };
  return { ...f, manifest, releases, calls, latestTag: () => latestTag,
    publish: (overrides = {}) => publishRelease({ env: { ...env, GITHUB_REPOSITORY: repository }, rootDirectory: f.directory, temporaryDirectory, run, ...overrides }) };
}
test('publish exposes a complete stable build, preserves the installed update URL, then advances Latest', t => {
  const f = publicationFixture(t); f.publish();
  const tag = `build-${sourceSha.slice(0, 12)}`;
  assert.equal(f.releases[tag].draft, false);
  assert.equal(f.releases[tag].prerelease, false);
  assert.equal(f.releases[tag].name, `HTTP Capture ${version}`);
  assert.equal(f.latestTag(), tag);
  assert.equal(f.releases['updater-preview'].prerelease, true);
  assert.deepEqual(f.releases['updater-preview'].manifest, f.manifest);
  const notes = readFileSync(path.join(f.directory, 'publish/notes.md'), 'utf8');
  assert(notes.includes(readReleaseNotes().summary));
  assert(notes.includes(releaseNotesPath));
  assert(notes.includes(`https://github.com/${repository}/blob/${sourceSha}/docs/roadmap.md`));
  assert.equal(f.manifest.notes, readReleaseNotes().summary);
  assert(f.calls.findIndex(args => args[1] === 'edit' && args[2] === tag) < f.calls.findIndex(args => args[1] === 'create' && args[2] === 'updater-preview'));
  const mutations = f.calls.filter(args => args[1] === 'create' || args[1] === 'edit');
  for (const args of mutations.filter(args => args[2] === 'updater-preview')) {
    assert(args.includes('--latest=false')); assert(args.includes('--prerelease'));
  }
  for (const args of mutations.filter(args => args[2] === tag)) assert(args.includes('--prerelease=false'));
  const latestEdit = f.calls.findIndex(args => args[1] === 'edit' && args.includes('--latest=true'));
  const channelEdit = f.calls.findIndex(args => args[1] === 'edit' && args[2] === 'updater-preview');
  assert(latestEdit > channelEdit);
  assert.equal(mutations.filter(args => args.includes('--latest=true')).length, 1);
});
test('release requires actual change notes before any GitHub mutation', t => {
  const f = publicationFixture(t);
  writeFileSync(path.join(f.directory, releaseNotesPath), '# Empty release\n');
  assert.throws(() => readReleaseNotes(f.directory), /更新内容/);
  assert.throws(() => f.publish(), /更新内容/);
  assert.equal(f.calls.length, 0);
});

test('rerun publishes the original immutable manifest and never replaces build assets', t => {
  const f = publicationFixture(t), tag = `build-${sourceSha.slice(0, 12)}`;
  const original = { ...f.manifest, notes: 'Original published build, independent of the rerun.' };
  f.releases[tag] = { draft: false, manifest: original }; f.publish();
  assert.deepEqual(f.releases['updater-preview'].manifest, original);
  assert.equal(f.calls.filter(args => ['create', 'upload'].includes(args[1]) && args[2] === tag).length, 0);
  const metadata = f.calls.filter(args => args[1] === 'edit' && args[2] === tag);
  assert.equal(metadata.length, 1);
  assert(metadata[0].includes('--latest=true')); assert(!metadata[0].includes('--notes-file'));
});
test('old successful builds cannot rewrite a newer channel and API errors are not treated as absence', t => {
  const f = publicationFixture(t), newer = structuredClone(f.manifest);
  newer.version = '0.1.100'; newer.source_sha = 'b'.repeat(40);
  for (const entry of Object.values(newer.platforms)) {
    entry.signature = signature(newer.version);
    entry.url = entry.url.replace(sourceSha.slice(0, 12), newer.source_sha.slice(0, 12));
  }
  f.releases['updater-preview'] = { draft: false, manifest: newer }; f.publish();
  assert.equal(f.calls.filter(args => ['create', 'upload', 'edit'].includes(args[1]) && args[2] === 'updater-preview').length, 0);
  assert.deepEqual(f.releases['updater-preview'].manifest, newer);
  assert.equal(f.calls.filter(args => args.includes('--latest=true')).length, 0);
  assert.equal(f.latestTag(), null);
  assert.throws(() => f.publish({ run: () => { throw Object.assign(new Error('API denied'), { stderr: 'gh: Bad credentials (HTTP 401)' }); } }), /API denied/);
});

test('a newer stable Latest cannot be overwritten when the compatibility channel lags behind', t => {
  const newerTag = `build-${'b'.repeat(12)}`;
  const f = publicationFixture(t, {}, newerTag), newer = structuredClone(f.manifest);
  newer.version = '0.1.100'; newer.source_sha = 'b'.repeat(40);
  for (const entry of Object.values(newer.platforms)) {
    entry.signature = signature(newer.version);
    entry.url = entry.url.replace(sourceSha.slice(0, 12), newer.source_sha.slice(0, 12));
  }
  f.releases[newerTag] = { draft: false, prerelease: false, manifest: newer };
  f.publish();
  assert.equal(f.latestTag(), newerTag);
  assert.equal(f.calls.filter(args => args.includes('--latest=true')).length, 0);
});

test('an identical rerun repairs failed Latest promotion without touching public build assets or the channel', t => {
  const tag = `build-${sourceSha.slice(0, 12)}`, legacyTag = 'legacy';
  const f = publicationFixture(t, {
    [legacyTag]: { draft: false, prerelease: false, name: 'HTTP Capture 0.1.0', target_commitish: 'c'.repeat(40) },
  }, legacyTag);
  f.releases[tag] = { draft: false, prerelease: false, manifest: f.manifest };
  f.releases['updater-preview'] = { draft: false, prerelease: true, manifest: f.manifest };
  f.publish();
  assert.equal(f.latestTag(), tag);
  const changes = f.calls.filter(args => ['create', 'upload', 'edit'].includes(args[1]));
  assert.equal(changes.length, 1);
  assert.deepEqual(changes[0], ['release', 'edit', tag, '--repo', repository, '--prerelease=false', '--latest=true']);
});

test('unknown legacy Latest identity fails before any public release mutation', t => {
  const f = publicationFixture(t, {
    legacy: { draft: false, prerelease: false, name: 'Unversioned package' },
  }, 'legacy');
  assert.throws(() => f.publish(), /Cannot determine/);
  assert.equal(f.calls.filter(args => ['create', 'upload', 'edit'].includes(args[1])).length, 0);
});
