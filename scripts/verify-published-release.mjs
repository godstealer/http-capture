import { createReadStream } from 'node:fs';
import { lstatSync, readFileSync, readdirSync } from 'node:fs';
import { createHash, createPublicKey, verify } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual, parseArgs } from 'node:util';
import { generateUpdater } from './generate-updater.mjs';
import {
  assetPath, assertRepository, assertSourceSha, assertVersion, buildTag, platforms,
  projectRoot, publicKey, readJson, signatureContent, validateManifest,
} from './release-updater.mjs';

function requireCondition(condition, message) {
  if (!condition) throw new Error(message);
}

/** Read only public trust material from the exact published source commit. */
export function committedReleaseTrust(sourceSha) {
  assertSourceSha(sourceSha);
  function committed(file) {
    try {
      return execFileSync('git', ['show', `${sourceSha}:${file}`], {
        cwd: projectRoot, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true,
      }).replace(/^\uFEFF/, '');
    } catch {
      throw new Error(`Cannot read committed public release configuration: ${file} at ${sourceSha}`);
    }
  }
  const key = publicKey(JSON.parse(committed('apps/desktop/src-tauri/tauri.conf.json')));
  const releaseScript = committed('scripts/release-updater.mjs');
  const notesPath = /export\s+const\s+releaseNotesPath\s*=\s*(['"])(docs\/release-\d{4}-\d{2}-\d{2}\.md)\1\s*;/.exec(releaseScript)?.[2];
  requireCondition(notesPath, 'Cannot identify the committed release notes document');
  const notes = committed(notesPath).trim().split(/^## 更新内容\r?$/m)[1]?.split(/^## /m)[0].trim();
  requireCondition(notes, 'Committed release notes require a non-empty 更新内容 section');
  return { publicKey: key, notes };
}

async function digest(file, algorithm = 'sha256') {
  const hash = createHash(algorithm);
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest();
}

function exactlyOne(names, label, suffix) {
  const matches = names.filter(name => name.startsWith(`${label}-`) && name.endsWith(suffix));
  requireCondition(matches.length === 1, `Expected exactly one ${label} ${suffix} published asset`);
  return matches[0];
}

/**
 * The CLI always gets trust from git. The second argument allows offline tests
 * to use freshly generated public keys without accessing any signing secret.
 */
export async function verifyPublishedRelease(options, trust) {
  const { directory, version, sourceSha, repository, channelFile } = options;
  assertVersion(version); assertSourceSha(sourceSha); assertRepository(repository);
  requireCondition(typeof directory === 'string' && directory, 'A downloaded release directory is required');
  requireCondition(typeof channelFile === 'string' && channelFile, 'A separately downloaded compatibility channel file is required');
  const root = path.resolve(directory), channelPath = path.resolve(channelFile);
  const publicTrust = trust ?? committedReleaseTrust(sourceSha);
  requireCondition(typeof publicTrust.notes === 'string' && publicTrust.notes.trim(), 'Expected committed release notes');
  const key = publicKey({ plugins: { updater: { pubkey: publicTrust.publicKey } } });
  const entries = readdirSync(root, { withFileTypes: true });
  const names = [];
  for (const entry of entries) {
    requireCondition(entry.isFile() && !entry.isSymbolicLink(), `Unexpected non-regular release entry: ${entry.name}`);
    if (path.resolve(root, entry.name) === channelPath) continue;
    assetPath(root, entry.name);
    requireCondition(lstatSync(path.join(root, entry.name)).size > 0, `Empty published asset: ${entry.name}`);
    names.push(entry.name);
  }
  requireCondition(names.length === 22, `Expected exactly 22 published assets, found ${names.length}`);
  requireCondition(channelPath !== path.join(root, 'latest.json'), 'Download the compatibility channel to a separate file');
  requireCondition(lstatSync(channelPath).isFile() && !lstatSync(channelPath).isSymbolicLink(), 'Compatibility channel must be a regular file');

  const manifest = validateManifest(readJson(assetPath(root, 'latest.json')), repository);
  requireCondition(manifest.version === version && manifest.source_sha === sourceSha, 'Published manifest differs from the expected version/source identity');
  const publishedAt = options.publishedAt ?? manifest.pub_date;
  requireCondition(manifest.pub_date === publishedAt, 'Published manifest date differs from the expected source Check date');
  const channel = validateManifest(readJson(channelPath), repository);
  requireCondition(isDeepStrictEqual(channel, manifest), 'Compatibility channel differs from the published build manifest');

  const expectedNames = new Set(['latest.json']);
  const checksumFiles = Object.keys(platforms).map(label => `SHA256SUMS-${label}.txt`);
  for (const name of checksumFiles) expectedNames.add(name);
  for (const [label, platform] of Object.entries(platforms)) {
    const metadataName = `updater-${label}.json`;
    expectedNames.add(metadataName);
    const metadata = readJson(assetPath(root, metadataName));
    for (const artifact of [metadata, ...Object.values(metadata.extraUpdaters ?? {})]) {
      assetPath(root, artifact.file); assetPath(root, artifact.signatureFile);
      expectedNames.add(artifact.file); expectedNames.add(artifact.signatureFile);
    }
    for (const suffix of platform.installers) expectedNames.add(exactlyOne(names, label, suffix));
  }
  expectedNames.add(exactlyOne(names, 'linux-x64', '-portable.tar.gz'));
  requireCondition(expectedNames.size === 22 && isDeepStrictEqual([...expectedNames].sort(), [...names].sort()), 'Published asset inventory does not match the complete four-platform release');

  const covered = new Set();
  for (const [index, checksumFile] of checksumFiles.entries()) {
    const label = Object.keys(platforms)[index];
    const lines = readFileSync(assetPath(root, checksumFile), 'utf8').trim().split(/\r?\n/);
    for (const line of lines) {
      const match = /^([a-f0-9]{64})  ([A-Za-z0-9][A-Za-z0-9._-]*)$/i.exec(line);
      requireCondition(match, `Malformed checksum line in ${checksumFile}`);
      const [, expectedHash, name] = match;
      const file = assetPath(root, name);
      requireCondition(expectedNames.has(name) && name !== 'latest.json' && !checksumFiles.includes(name), `Unexpected checksum target: ${name}`);
      requireCondition(name.startsWith(`${label}-`) || name === `updater-${label}.json`, `Checksum target belongs to another platform: ${name}`);
      requireCondition(!covered.has(name), `Duplicate checksum target: ${name}`);
      covered.add(name);
      requireCondition((await digest(file)).toString('hex') === expectedHash.toLowerCase(), `SHA256 mismatch: ${name}`);
    }
  }
  const expectedCoverage = [...expectedNames].filter(name => name !== 'latest.json' && !checksumFiles.includes(name)).sort();
  requireCondition(covered.size === 17 && isDeepStrictEqual([...covered].sort(), expectedCoverage), 'Checksums must cover all 17 package/signature/metadata assets exactly once');

  // Reuse the CI validator without its CLI, which would rewrite latest.json.
  // The expected public notes are pinned to the source commit, not this checkout.
  const generated = generateUpdater({ directory: root, version, sourceSha, repository, publishedAt, updaterPublicKey: key });
  requireCondition(isDeepStrictEqual({ ...generated, notes: publicTrust.notes }, manifest), 'Published manifest does not match the verified packages and committed notes');

  const keyPacket = Buffer.from(Buffer.from(key, 'base64').toString('utf8').trim().split(/\r?\n/)[1], 'base64');
  requireCondition(keyPacket.subarray(0, 2).toString('ascii') === 'Ed', 'Unsupported updater public key algorithm');
  const verificationKey = createPublicKey({
    key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), keyPacket.subarray(10)]),
    format: 'der', type: 'spki',
  });
  let signaturesVerified = 0;
  for (const [target, artifact] of Object.entries(manifest.platforms)) {
    const name = decodeURIComponent(new URL(artifact.url).pathname.split('/').at(-1));
    const file = assetPath(root, name);
    const content = signatureContent(readFileSync(`${file}.sig`, 'utf8'), version, key);
    requireCondition(content === artifact.signature, `Manifest signature differs from its .sig asset: ${target}`);
    const lines = Buffer.from(content, 'base64').toString('utf8').trim().split(/\r?\n/);
    const packet = Buffer.from(lines[1], 'base64');
    requireCondition(packet.subarray(0, 2).toString('ascii') === 'ED', `Unsupported updater signature algorithm for ${target}; expected prehashed ED`);
    const signature = packet.subarray(10);
    const comment = Buffer.from(lines[2].slice('trusted comment: '.length), 'utf8');
    requireCondition(verify(null, Buffer.concat([signature, comment]), verificationKey, Buffer.from(lines[3], 'base64')), `Trusted-comment signature verification failed: ${target}`);
    requireCondition(verify(null, await digest(file, 'blake2b512'), verificationKey, signature), `Package signature verification failed: ${target}`);
    signaturesVerified++;
  }
  return { version, sourceSha, tag: buildTag(sourceSha), publishedAt, assetCount: names.length,
    checksumCount: covered.size, updaterTargetCount: Object.keys(manifest.platforms).length,
    signaturesVerified, channelMatches: true };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const { values } = parseArgs({ options: {
      directory: { type: 'string' }, version: { type: 'string' }, 'source-sha': { type: 'string' },
      'published-at': { type: 'string' }, repository: { type: 'string' }, 'channel-file': { type: 'string' },
      help: { type: 'boolean' },
    } });
    if (values.help) console.log('node scripts/verify-published-release.mjs --directory <download-dir> --version <0.1.N> --source-sha <full-sha> --repository <owner/repo> --channel-file <channel.json> [--published-at <source-Check-UTC-time>]');
    else console.log(JSON.stringify(await verifyPublishedRelease({ directory: values.directory, version: values.version,
      sourceSha: values['source-sha'], publishedAt: values['published-at'], repository: values.repository,
      channelFile: values['channel-file'] }), null, 2));
  } catch (error) {
    console.error(`Published release verification failed: ${error.message}`);
    process.exitCode = 1;
  }
}
