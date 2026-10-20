import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync, randomBytes, sign, verify as verifySignature } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { collectRelease } from './collect-release.mjs';
import { generateUpdater } from './generate-updater.mjs';
import { platforms, projectRoot, readReleaseNotes, releaseNotesPath } from './release-updater.mjs';
import { releaseNotesDocumentPath, verifyPublishedRelease } from './verify-published-release.mjs';

const version = '0.1.19', sourceSha = 'a'.repeat(40), repository = 'godstealer/http-capture';
const publishedAt = '2026-10-18T14:49:43Z';
const sha256 = file => createHash('sha256').update(readFileSync(file)).digest('hex');

test('committed release notes paths accept dated documents with restricted lowercase suffixes', () => {
  for (const document of ['docs/release-2026-10-20.md', 'docs/release-2026-10-20-response-rewrite.md', 'docs/release-2026-10-20-p0.md']) {
    for (const quote of ["'", '"']) {
      assert.equal(releaseNotesDocumentPath(`export const releaseNotesPath = ${quote}${document}${quote};`), document);
    }
  }
  assert.equal(releaseNotesDocumentPath(readFileSync(path.join(projectRoot, 'scripts/release-updater.mjs'), 'utf8')), releaseNotesPath);
  assert.ok(readReleaseNotes().summary);
});

test('committed release notes extraction rejects traversal, dynamic paths and ambiguous declarations', () => {
  for (const document of [
    '../docs/release-2026-10-20.md', 'docs/../release-2026-10-20.md', '/docs/release-2026-10-20.md',
    'docs/release-2026-10-20-../private.md', 'docs/release-2026-10-20-response/rewrite.md',
    'docs/release-2026-10-20-response%2frewrite.md', 'docs/release-2026-10-20-Response.md',
    'docs/release-2026-10-20-response_rewrite.md', 'docs/release-2026-10-20--response.md',
    'docs/release-2026-10-20-.md', 'docs/release-2026-10-20-response.md.backup',
    'https://example.invalid/docs/release-2026-10-20.md', 'docs\\release-2026-10-20.md',
    'docs/release-2026-10-20\u0000.md',
  ]) {
    assert.throws(() => releaseNotesDocumentPath(`export const releaseNotesPath = '${document}';`), /document/);
  }
  const valid = "export const releaseNotesPath = 'docs/release-2026-10-20.md';";
  for (const source of [undefined, '', 'const releaseNotesPath = "docs/release-2026-10-20.md";',
    'export const releaseNotesPath = `docs/release-2026-10-20.md`;',
    'export const releaseNotesPath = "docs/release-2026-10-20" + ".md";', valid + '\n' + valid]) {
    assert.throws(() => releaseNotesDocumentPath(source), /document/);
  }
});

function fixture(t) {
  const parent = path.join(projectRoot, '.local'); mkdirSync(parent, { recursive: true });
  const directory = mkdtempSync(path.join(parent, 'published-release-test-'));
  t.after(() => {
    assert.equal(path.dirname(path.resolve(directory)), path.resolve(parent));
    assert(path.basename(directory).startsWith('published-release-test-'));
    rmSync(directory, { recursive: true, force: true });
  });
  const download = path.join(directory, 'downloads'); mkdirSync(download);
  // These keys exist only in this test's memory. No local signing key is read.
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const keyId = randomBytes(8);
  const rawKey = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const keyPacket = Buffer.concat([Buffer.from('Ed'), keyId, rawKey]);
  const key = Buffer.from(`untrusted comment: disposable offline test key\n${keyPacket.toString('base64')}\n`).toString('base64');
  function signature(file) {
    const digest = createHash('blake2b512').update(readFileSync(file)).digest();
    const detached = sign(null, digest, privateKey);
    const packet = Buffer.concat([Buffer.from('ED'), keyId, detached]);
    const comment = `timestamp:1\tfile:${path.basename(file)}\tversion:${version}`;
    const global = sign(null, Buffer.concat([detached, Buffer.from(comment)]), privateKey);
    return Buffer.from(`untrusted comment: disposable signature\n${packet.toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`).toString('base64');
  }
  const metadata = {};
  for (const [label, platform] of Object.entries(platforms)) {
    const bundle = path.join(directory, label); mkdirSync(bundle);
    const suffixes = new Set([...platform.installers, platform.updater, ...(platform.additionalUpdaters ?? []).map(item => item.suffix)]);
    for (const suffix of suffixes) writeFileSync(path.join(bundle, `HTTP-Capture_${version}${suffix}`), Buffer.from(`${label}:${suffix}\0binary\xff`, 'latin1'));
    for (const suffix of [platform.updater, ...(platform.additionalUpdaters ?? []).map(item => item.suffix)]) {
      const file = path.join(bundle, `HTTP-Capture_${version}${suffix}`);
      writeFileSync(`${file}.sig`, signature(file));
    }
    metadata[label] = collectRelease({ label, bundleDirectory: bundle, outputDirectory: download, version,
      sourceSha, updaterPublicKey: key, portable: false, runtime: { platform: platform.os, arch: platform.arch } });
  }
  const portableName = `linux-x64-HTTP-Capture_${version}-portable.tar.gz`;
  writeFileSync(path.join(download, portableName), 'offline portable fixture');
  const linuxSums = path.join(download, 'SHA256SUMS-linux-x64.txt');
  writeFileSync(linuxSums, readFileSync(linuxSums, 'utf8') + `${sha256(path.join(download, portableName))}  ${portableName}\n`);
  const manifest = generateUpdater({ directory: download, version, sourceSha, repository, publishedAt, updaterPublicKey: key });
  writeFileSync(path.join(download, 'latest.json'), JSON.stringify(manifest, null, 2));
  const channelFile = path.join(download, 'channel.json');
  writeFileSync(channelFile, JSON.stringify(manifest, null, 2));
  const options = { directory: download, version, sourceSha, repository, publishedAt, channelFile };
  const trust = { publicKey: key, notes: readReleaseNotes().summary };
  function rehash(name) {
    let matched = false;
    for (const label of Object.keys(platforms)) {
      const file = path.join(download, `SHA256SUMS-${label}.txt`);
      const lines = readFileSync(file, 'utf8').trim().split('\n').map(line => {
        if (line.endsWith(`  ${name}`)) { matched = true; return `${sha256(path.join(download, name))}  ${name}`; }
        return line;
      });
      writeFileSync(file, lines.join('\n') + '\n');
    }
    assert(matched, `No checksum target ${name}`);
  }
  function changeSignature(transform) {
    const meta = metadata['windows-x64'];
    const file = path.join(download, meta.signatureFile);
    const lines = Buffer.from(readFileSync(file, 'utf8'), 'base64').toString('utf8').trim().split(/\r?\n/);
    transform(lines);
    const changed = Buffer.from(lines.join('\n') + '\n').toString('base64');
    writeFileSync(file, changed);
    rehash(meta.signatureFile);
    manifest.platforms['windows-x86_64'].signature = changed;
    writeFileSync(path.join(download, 'latest.json'), JSON.stringify(manifest));
    writeFileSync(channelFile, JSON.stringify(manifest));
  }
  function reSignComment(transform) {
    let cryptographicallyValid = false;
    changeSignature(lines => {
      lines[2] = transform(lines[2]);
      const detached = Buffer.from(lines[1], 'base64').subarray(10);
      const payload = Buffer.concat([detached, Buffer.from(lines[2].slice('trusted comment: '.length))]);
      const signature = sign(null, payload, privateKey);
      cryptographicallyValid = verifySignature(null, payload, publicKey, signature);
      lines[3] = signature.toString('base64');
    });
    return cryptographicallyValid;
  }
  return { directory, download, metadata, manifest, options, trust, keyPacket, rehash, changeSignature, reSignComment,
    verify: (changes = {}, changedTrust = trust) => verifyPublishedRelease({ ...options, ...changes }, changedTrust) };
}

test('complete public inventory verifies all checksums and five real package/comment signatures without modifying files', async t => {
  const f = fixture(t);
  const before = Object.fromEntries(readdirSync(f.download).map(name => [name, sha256(path.join(f.download, name))]));
  assert.deepEqual(await f.verify(), { version, sourceSha, tag: `build-${sourceSha.slice(0, 12)}`, publishedAt,
    assetCount: 22, checksumCount: 17, updaterTargetCount: 5, signaturesVerified: 5, channelMatches: true });
  assert.deepEqual(Object.fromEntries(readdirSync(f.download).map(name => [name, sha256(path.join(f.download, name))])), before);
  assert.equal((await f.verify({ publishedAt: undefined })).publishedAt, publishedAt);
});

test('changed package is rejected even after an attacker updates its SHA256 and metadata', async t => {
  const f = fixture(t), meta = f.metadata['windows-x64'];
  writeFileSync(path.join(f.download, meta.file), 'tampered package');
  const name = 'updater-windows-x64.json';
  writeFileSync(path.join(f.download, name), JSON.stringify({ ...meta, sha256: sha256(path.join(f.download, meta.file)) }));
  f.rehash(meta.file); f.rehash(name);
  await assert.rejects(f.verify(), /Package signature verification failed: windows-x86_64/);
});

test('trusted comment is cryptographically verified even when manifest and .sig text agree', async t => {
  const f = fixture(t);
  f.changeSignature(lines => { lines[2] = lines[2].replace('timestamp:1', 'timestamp:2'); });
  await assert.rejects(f.verify(), /Trusted-comment signature verification failed/);
});

for (const [line, label] of [[1, 'packet'], [3, 'global']]) {
  test(`signature ${label} rejects an ignored Base64 character even after checksums and manifests are updated`, async t => {
    const f = fixture(t);
    f.changeSignature(lines => { lines[line] = lines[line].slice(0, 4) + '!' + lines[line].slice(4); });
    await assert.rejects(f.verify(), /canonical base64/);
  });
}

test('signed version cannot be replaced by editing the public envelope', async t => {
  const f = fixture(t);
  f.changeSignature(lines => { lines[2] = lines[2].replace(`version:${version}`, 'version:0.1.20'); });
  await assert.rejects(f.verify(), /matching signed app version/);
});

for (const [label, transform] of [
  ['leading', text => text.replace('\tversion:', '\t version:')],
  ['trailing', text => text.replace(`version:${version}`, `version:${version} `)],
]) {
  test(`a genuinely re-signed comment with ${label} version whitespace is rejected like the desktop updater`, async t => {
    const f = fixture(t);
    assert.equal(f.reSignComment(transform), true, 'Modified trusted-comment global signature must remain cryptographically valid');
    await assert.rejects(f.verify(), /matching signed app version/);
  });
}

test('matching key id does not allow a substituted public key', async t => {
  const f = fixture(t), fake = Buffer.from(f.keyPacket); fake[10] ^= 1;
  const key = Buffer.from(`untrusted comment: wrong public key\n${fake.toString('base64')}\n`).toString('base64');
  await assert.rejects(f.verify({}, { ...f.trust, publicKey: key }), /signature verification failed/);
});

test('missing and unexpected release attachments are rejected', async t => {
  const f = fixture(t), extra = path.join(f.download, 'unexpected.txt');
  writeFileSync(extra, 'not part of the public release');
  await assert.rejects(f.verify(), /exactly 22/);
  unlinkSync(extra); unlinkSync(path.join(f.download, f.metadata['windows-x64'].file));
  await assert.rejects(f.verify(), /exactly 22/);
});

test('all 17 checksum targets are required, without duplicates or unsafe paths', async t => {
  const f = fixture(t), file = path.join(f.download, 'SHA256SUMS-windows-x64.txt');
  const original = readFileSync(file, 'utf8'), lines = original.trim().split('\n');
  writeFileSync(file, lines.slice(1).join('\n') + '\n');
  await assert.rejects(f.verify(), /all 17/);
  writeFileSync(file, original + lines[0] + '\n');
  await assert.rejects(f.verify(), /Duplicate checksum target/);
  writeFileSync(file, original + `${'a'.repeat(64)}  ../outside.key\n`);
  await assert.rejects(f.verify(), /Malformed checksum line/);
});

test('manifest, source identity, source date and compatibility channel must agree', async t => {
  const f = fixture(t);
  await assert.rejects(f.verify({ sourceSha: 'b'.repeat(40) }), /version\/source identity/);
  await assert.rejects(f.verify({ version: '0.1.20' }), /version\/source identity/);
  await assert.rejects(f.verify({ publishedAt: '2026-10-19T01:00:00Z' }), /source Check date/);
  writeFileSync(f.options.channelFile, JSON.stringify({ ...f.manifest, notes: 'different notes' }));
  await assert.rejects(f.verify(), /Compatibility channel differs/);
});

test('committed notes are checked independently of the public manifest', async t => {
  const f = fixture(t);
  await assert.rejects(f.verify({}, { ...f.trust, notes: 'different committed change notes' }), /committed notes/);
});

test('unknown or legacy signature algorithm fails explicitly instead of claiming verification', async t => {
  const f = fixture(t);
  f.changeSignature(lines => {
    const packet = Buffer.from(lines[1], 'base64'); packet.write('Ed'); lines[1] = packet.toString('base64');
  });
  await assert.rejects(f.verify(), /Unsupported updater signature algorithm.*expected prehashed ED/);
});
