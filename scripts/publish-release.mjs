import { execFileSync } from 'node:child_process';
import { mkdtempSync, readdirSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { assetPath, assertRepository, assertVersion, buildTag, projectRoot, readJson, releaseSettings, shouldPromote, validateManifest, readReleaseNotes, releaseNotesPath } from './release-updater.mjs';

// Only invoked by the gated CI publish job. Never reads signing secrets.
export function publishRelease({ env = process.env, rootDirectory = projectRoot, run = args => execFileSync('gh', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }), temporaryDirectory = mkdtempSync(path.join(os.tmpdir(), 'http-capture-release-')) } = {}) {
  const repository = assertRepository(env.GITHUB_REPOSITORY);
  const settings = releaseSettings(readJson(path.join(rootDirectory, 'apps/desktop/src-tauri/tauri.conf.json')), env);
  const directory = path.join(rootDirectory, 'release-assets');
  const manifestPath = assetPath(directory, 'latest.json');
  let candidate = validateManifest(readJson(manifestPath), repository);
  if (candidate.version !== settings.version || candidate.source_sha !== settings.sourceSha) throw new Error('Updater manifest differs from the successful source Check');
  const tag = buildTag(settings.sourceSha);
  function releaseInfo(suffix) {
    try { return JSON.parse(run(['api', `repos/${repository}/releases/${suffix}`])); }
    catch (error) {
      if (/HTTP 404\b/.test(String(error.stderr))) return null;
      throw error;
    }
  }
  const info = releaseTag => releaseInfo(`tags/${releaseTag}`);
  function downloadManifest(releaseTag, destination) {
    run(['release', 'download', releaseTag, '--repo', repository, '--pattern', 'latest.json', '--output', destination]);
    return validateManifest(readJson(destination), repository);
  }
  const notes = path.join(temporaryDirectory, 'notes.md');
  const details = readReleaseNotes(rootDirectory).markdown.replace(/\]\(([\w-]+\.md)\)/g,
    (_, file) => `](https://github.com/${repository}/blob/${settings.sourceSha}/docs/${file})`);
  writeFileSync(notes, `HTTP Capture ${settings.version} · ${settings.sourceSha}\n\n` +
    '本提交的 Check 与四平台构建已通过；签名更新包和 SHA256 校验文件随附件发布。\n\n' +
    details + '\n\n' +
    `源码中的发布说明：https://github.com/${repository}/blob/${settings.sourceSha}/${releaseNotesPath}\n`);
  const existing = info(tag);
  if (existing && !existing.draft) {
    // Reruns may have newly randomized signatures. Only the already-published
    // manifest can point at this immutable tag; never advertise rebuilt bytes.
    candidate = downloadManifest(tag, path.join(temporaryDirectory, 'published.json'));
    if (candidate.version !== settings.version || candidate.source_sha !== settings.sourceSha) throw new Error('Immutable build tag belongs to a different release version or commit');
  }
  // Keep this endpoint for installed versions that already trust its fixed URL.
  // The channel stays a prerelease; downloadable builds are stable releases.
  const channel = 'updater-preview';
  const currentRelease = info(channel);
  let current = null;
  if (currentRelease && !currentRelease.draft) current = downloadManifest(channel, path.join(temporaryDirectory, 'current.json'));
  const promoteChannel = shouldPromote(candidate, current);
  const channelMatches = current?.version === candidate.version;

  // A previous attempt can publish GitHub Latest and then fail to update the
  // channel (or vice versa). Check both pointers before changing either Latest.
  const latestRelease = releaseInfo('latest');
  let latest = null;
  if (latestRelease) {
    if (latestRelease.draft || latestRelease.prerelease) throw new Error('GitHub Latest must identify a public stable release');
    if (latestRelease.assets?.some(asset => asset.name === 'latest.json')) {
      latest = downloadManifest(latestRelease.tag_name, path.join(temporaryDirectory, 'stable.json'));
    } else {
      // Early releases predate the updater manifest. Accept their explicit
      // numeric title only; an unknown release must not be silently displaced.
      const version = /^HTTP Capture (\d+\.\d+\.\d+)$/.exec(latestRelease.name ?? '')?.[1];
      if (!version) throw new Error('Cannot determine the current stable release version');
      latest = { version: assertVersion(version), source_sha: latestRelease.target_commitish };
    }
  }
  const promoteLatest = shouldPromote(candidate, latest);

  if (!existing || existing.draft) {
    if (!existing) run(['release', 'create', tag, '--repo', repository, '--target', settings.sourceSha, '--title', `HTTP Capture ${settings.version}`, '--notes-file', notes, '--draft', '--prerelease=false', '--latest=false']);
    const assets = readdirSync(directory, { withFileTypes: true }).filter(e => e.isFile()).map(e => assetPath(directory, e.name));
    run(['release', 'upload', tag, '--repo', repository, ...assets, '--clobber']);
    run(['release', 'edit', tag, '--repo', repository, '--title', `HTTP Capture ${settings.version}`, '--notes-file', notes, '--draft=false', '--prerelease=false', '--latest=false']);
  }
  if (promoteChannel) {
    // The workflow serializes this whole job across source commits. No older
    // build can race a newer promotion, and the comparator prevents rollback.
    if (!currentRelease) run(['release', 'create', channel, '--repo', repository, '--target', settings.sourceSha, '--title', 'HTTP Capture update channel', '--notes-file', notes, '--draft', '--prerelease', '--latest=false']);
    const channelManifest = path.join(temporaryDirectory, 'latest.json');
    writeFileSync(channelManifest, JSON.stringify(candidate, null, 2) + '\n');
    run(['release', 'upload', channel, '--repo', repository, channelManifest, '--clobber']);
    run(['release', 'edit', channel, '--repo', repository, '--title', 'HTTP Capture update channel', '--notes-file', notes, '--draft=false', '--prerelease', '--latest=false']);
    console.log(`Compatibility update channel advanced to ${candidate.version}`);
  } else {
    console.log(`Kept compatibility update channel at ${current?.version}; candidate ${candidate.version} is not newer`);
  }
  // Promote only after the compatible manifest is public. An identical rerun
  // repairs a failed final metadata edit without uploading immutable assets.
  if ((promoteChannel || channelMatches) && promoteLatest) {
    run(['release', 'edit', tag, '--repo', repository, '--prerelease=false', '--latest=true']);
    console.log(`Stable latest advanced to ${candidate.version}`);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) publishRelease();
