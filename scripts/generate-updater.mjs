import { readFileSync, writeFileSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { platforms, assetPath, assertVersion, assertSourceSha, assertRepository, buildTag, signatureContent, publicKey, readJson, projectRoot, releaseSettings, validateManifest, readReleaseNotes } from './release-updater.mjs';
import { sha256 } from './collect-release.mjs';

export function generateUpdater({ directory, version, sourceSha, repository, publishedAt, updaterPublicKey }) {
  assertVersion(version); assertSourceSha(sourceSha); assertRepository(repository);
  if (!/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z$/.test(publishedAt ?? '') || !Number.isFinite(Date.parse(publishedAt))) throw new Error('Expected source Check creation time in UTC RFC 3339');
  const entries = {};
  for (const [label, platform] of Object.entries(platforms)) {
    const metadata = readJson(assetPath(directory, `updater-${label}.json`));
    if (metadata.schemaVersion !== 1 || metadata.label !== label || metadata.target !== platform.target || metadata.version !== version || metadata.sourceSha !== sourceSha) throw new Error(`Mismatched ${label} updater build identity`);
    const expectedExtra = (platform.additionalUpdaters ?? []).map(extra => extra.target).sort();
    if (JSON.stringify(Object.keys(metadata.extraUpdaters ?? {}).sort()) !== JSON.stringify(expectedExtra)) throw new Error(`Missing or unexpected ${label} updater targets`);
    for (const [target, suffix, artifact] of [[platform.target, platform.updater, metadata], ...(platform.additionalUpdaters ?? []).map(extra => [extra.target, extra.suffix, metadata.extraUpdaters[extra.target]])]) {
      const file = assetPath(directory, artifact.file);
      const signatureFile = assetPath(directory, artifact.signatureFile);
      if (!artifact.file.startsWith(`${label}-`) || !artifact.file.endsWith(suffix) || artifact.signatureFile !== `${artifact.file}.sig`) throw new Error(`Invalid ${label} updater artifact path`);
      if (!statSync(file).isFile() || !statSync(file).size || sha256(file) !== artifact.sha256) throw new Error(`Invalid ${label} updater artifact checksum`);
      const signature = signatureContent(readFileSync(signatureFile, 'utf8'), version, updaterPublicKey);
      entries[target] = { signature, url: `https://github.com/${repository}/releases/download/${buildTag(sourceSha)}/${encodeURIComponent(artifact.file)}` };
    }
  }
  const notes = readReleaseNotes().summary;
  return validateManifest({ version, notes, pub_date: publishedAt, platforms: entries, source_sha: sourceSha }, repository);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const config = readJson(path.join(projectRoot, 'apps/desktop/src-tauri/tauri.conf.json'));
  const settings = releaseSettings(config, process.env);
  const directory = path.join(projectRoot, 'release-assets');
  const manifest = generateUpdater({ directory, ...settings, repository: process.env.GITHUB_REPOSITORY, publishedAt: process.env.SOURCE_CHECK_CREATED_AT, updaterPublicKey: publicKey(config) });
  writeFileSync(path.join(directory, 'latest.json'), JSON.stringify(manifest, null, 2) + '\n');
  console.log(`Verified all four signed updater artifacts for ${manifest.version}`);
}
