import test, { after } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, existsSync, lstatSync, readlinkSync, rmSync, chmodSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { gzipSync } from 'node:zlib';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Every installer invocation sees only synthetic releases, a private HOME and a
// curl replacement. Real tar and SHA256 tools still validate/extract the bytes.
const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bash = process.env.INSTALL_TEST_BASH || (process.platform === 'win32'
  ? ['D:/Program Files/Git/bin/bash.exe', 'C:/Program Files/Git/bin/bash.exe'].find(existsSync)
  : 'bash');
assert(bash, 'Set INSTALL_TEST_BASH to a Bash executable (Git Bash on Windows).');
const temporaryRoot = mkdtempSync(path.join(tmpdir(), 'http-capture-install-test-'));
const isolatedHome = path.join(temporaryRoot, 'bootstrap-home'); mkdirSync(isolatedHome);
const bootstrapEnv = { ...process.env, HOME: isolatedHome, BASH_ENV: '', ENV: '', MSYS_NO_PATHCONV: '1' };
function shell(args, options = {}) {
  const fixtureArguments = options.env?.INSTALL_FIXTURE_PATH ? [
    '-c', 'export PATH="$INSTALL_FIXTURE_PATH"; [ "$(command -v curl)" = "$INSTALL_FIXTURE_ROOT/mocks/curl" ] || exit 93; exec "$BASH" "$@"',
    'install-fixture', ...args,
  ] : args;
  const result = spawnSync(bash, ['--noprofile', '--norc', ...fixtureArguments], {
    cwd: temporaryRoot, encoding: 'utf8', timeout: 30000,
    maxBuffer: 2 * 1024 * 1024, env: bootstrapEnv, ...options,
  });
  if (result.error) throw result.error;
  return result;
}
function checked(args, options) {
  const result = shell(args, options);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  return result.stdout.trim();
}
function toShell(value) {
  return process.platform === 'win32'
    ? checked(['-c', 'cygpath -u "$1"', 'install-test', value])
    : value;
}
const temporaryShellRoot = toShell(temporaryRoot);
const installerSnapshot = path.join(temporaryRoot, 'install.sh');
writeFileSync(installerSnapshot, readFileSync(path.join(projectRoot, 'install.sh')), { mode: 0o755 });
const installer = toShell(installerSnapshot);
const basePath = checked(['-c', 'printf "%s" "$PATH"']);
const realMv = checked(['-c', 'command -v mv']);
const tag = 'build-0123456789ab';
const repo = 'https://github.com/godstealer/http-capture';
let fixtureNumber = 0;

after(() => {
  // Delete only this invocation's fresh OS-temporary test tree.
  assert.equal(path.dirname(path.resolve(temporaryRoot)), path.resolve(tmpdir()));
  assert(path.basename(temporaryRoot).startsWith('http-capture-install-test-'));
  rmSync(temporaryRoot, { recursive: true, force: true });
});

const mockCurl = `#!/usr/bin/env bash
set -eu
printf '%s\\n' "$*" >> "$INSTALL_FIXTURE_CURL_LOG"
output=''; url=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) output=$2; shift 2 ;;
    --write-out|--retry|--connect-timeout|--max-time|--proto|--proto-redir) shift 2 ;;
    --fail|--silent|--show-error|--location|--head) shift ;;
    https://*) url=$1; shift ;;
    *) printf 'Unexpected mock curl argument\\n' >&2; exit 79 ;;
  esac
done
case "$url" in
  '${repo}/releases/latest') printf '%s' "$INSTALL_FIXTURE_RESOLVED" ;;
  '${repo}/releases/download/'*/SHA256SUMS-*.txt)
    [ "\${INSTALL_FIXTURE_FAIL:-}" != checksums ] || exit 22
    cp "$INSTALL_FIXTURE_ROOT/\${url##*/}" "$output" ;;
  '${repo}/releases/download/'*/*.tar.gz)
    if [ "\${INSTALL_FIXTURE_FAIL:-}" = asset ]; then
      printf 'partial' > "$output"; exit 22
    fi
    cp "$INSTALL_FIXTURE_ROOT/package.tar.gz" "$output" ;;
  *) printf 'Unexpected URL blocked by offline curl fixture\\n' >&2; exit 78 ;;
esac
`;
const mockUname = `#!/usr/bin/env bash
case "$1" in -s) printf '%s\\n' "$INSTALL_FIXTURE_OS" ;; -m) printf '%s\\n' "$INSTALL_FIXTURE_ARCH" ;; *) exit 80 ;; esac
`;
const mockSysctl = `#!/usr/bin/env bash
[ "$*" = '-n hw.optional.arm64' ] || exit 81
printf '%s\\n' "\${INSTALL_FIXTURE_ARM64:-0}"
`;
const mockCodesign = `#!/usr/bin/env bash
printf '%s\\n' "$*" >> "$INSTALL_FIXTURE_CODESIGN_LOG"
[ "\${INSTALL_FIXTURE_CODESIGN_FAIL:-0}" = 0 ]
`;

const mockMv = `#!/usr/bin/env bash
set -eu
if [ "\${INSTALL_FIXTURE_MOVE_FAIL:-0}" = 1 ] && [ "$#" = 2 ]; then
  case "$1" in
    */.http-capture-install.*/"HTTP Capture.app")
      case "$2" in */"HTTP Capture.app") printf 'Fixture move failure\\n' >&2; exit 41 ;; esac ;;
  esac
fi
exec "$INSTALL_FIXTURE_REAL_MV" "$@"
`;

function fixture({ os = 'Linux', arch = 'x86_64', arm64 = '0', name = 'prefix with spaces', label, defaultPrefix = false } = {}) {
  const basename = `case-${++fixtureNumber}`;
  const root = path.join(temporaryRoot, basename), shellRoot = `${temporaryShellRoot}/${basename}`;
  const home = path.join(root, 'home with spaces');
  const mockBin = path.join(root, 'mocks');
  const scratch = path.join(root, 'scratch');
  for (const directory of [root, home, mockBin, scratch]) mkdirSync(directory, { recursive: true });
  for (const [name, text] of Object.entries({ curl: mockCurl, uname: mockUname, sysctl: mockSysctl, codesign: mockCodesign, mv: mockMv })) {
    writeFileSync(path.join(mockBin, name), text, { mode: 0o755 });
  }
  label ??= os === 'Linux' ? 'linux-x64' : (arch === 'arm64' || arm64 === '1' ? 'macos-arm64' : 'macos-x64');
  const prefix = defaultPrefix ? path.join(home, label === 'linux-x64' ? '.local' : 'Applications') : path.join(root, name);
  const prefixShell = defaultPrefix ? `${shellRoot}/home with spaces/${label === 'linux-x64' ? '.local' : 'Applications'}` : `${shellRoot}/${name}`;
  const env = {
    ...bootstrapEnv, HOME: `${shellRoot}/home with spaces`, TMPDIR: `${shellRoot}/scratch`,
    XDG_CONFIG_HOME: `${shellRoot}/home with spaces/config`, XDG_DATA_HOME: `${shellRoot}/home with spaces/data`,
    PATH: `${shellRoot}/mocks:${basePath}`, INSTALL_FIXTURE_PATH: `${shellRoot}/mocks:${basePath}`, INSTALL_FIXTURE_ROOT: shellRoot,
    INSTALL_FIXTURE_CURL_LOG: `${shellRoot}/curl.log`, INSTALL_FIXTURE_CODESIGN_LOG: `${shellRoot}/codesign.log`,
    INSTALL_FIXTURE_OS: os, INSTALL_FIXTURE_ARCH: arch, INSTALL_FIXTURE_ARM64: arm64,
    INSTALL_FIXTURE_RESOLVED: `${repo}/releases/tag/${tag}`,
    INSTALL_FIXTURE_LAUNCH_LOG: `${shellRoot}/launch.log`, INSTALL_FIXTURE_REAL_MV: realMv,
  };
  checked(['-c', 'chmod 755 "$INSTALL_FIXTURE_ROOT/mocks/"*'], { env });
  return { root, shellRoot, home, prefix, prefixShell, defaultPrefix, label, env };
}
function packageBytes(f, bytes, checksum = createHash('sha256').update(bytes).digest('hex')) {
  f.asset = f.label === 'linux-x64'
    ? 'linux-x64-HTTP-Capture_0.1.82_amd64-portable.tar.gz'
    : `${f.label}-HTTP-Capture.app.tar.gz`;
  writeFileSync(path.join(f.root, 'package.tar.gz'), bytes);
  writeFileSync(path.join(f.root, `SHA256SUMS-${f.label}.txt`), `${checksum}  ${f.asset}\n`);
}
function normalPackage(f, { version = 'fixture version', executable = true } = {}) {
  const rootName = f.label === 'linux-x64' ? 'HTTP-Capture' : 'HTTP Capture.app';
  const source = path.join(f.root, 'source');
  const entry = path.join(source, rootName, f.label === 'linux-x64' ? 'AppRun' : 'Contents/MacOS/http-capture');
  mkdirSync(path.dirname(entry), { recursive: true });
  writeFileSync(entry, '#!/usr/bin/env bash\nprintf \'%s\\n\' "$@" > "$INSTALL_FIXTURE_LAUNCH_LOG"\n', { mode: 0o755 });
  writeFileSync(path.join(source, rootName, 'fixture-version.txt'), version);
  // BSD tar lacks GNU --mode. Unix chmod provides real execute-bit tests;
  // Git Bash uses a deterministic archive mode and skips unsupported negatives.
  if (process.platform !== 'win32') chmodSync(entry, executable ? 0o755 : 0o644);
  const command = process.platform === 'win32'
    ? 'tar --format=ustar --mode="$1" -czf "$INSTALL_FIXTURE_ROOT/package.tar.gz" -C "$INSTALL_FIXTURE_ROOT/source" "$2"'
    : 'tar --format=ustar -czf "$INSTALL_FIXTURE_ROOT/package.tar.gz" -C "$INSTALL_FIXTURE_ROOT/source" "$2"';
  checked(['-c', command, 'install-test', executable ? '0755' : '0644', rootName], { env: f.env });
  packageBytes(f, readFileSync(path.join(f.root, 'package.tar.gz')));
}
function run(f, { latest = false, args = [], env = {} } = {}) {
  return shell([installer, ...(latest ? [] : ['--tag', tag]), ...(f.defaultPrefix ? [] : ['--prefix', f.prefixShell]), ...args], { env: { ...f.env, ...env } });
}
function succeeds(result) { assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`); }
function fails(result, pattern) {
  assert.notEqual(result.status, 0, result.stdout);
  if (pattern) assert.match(`${result.stdout}\n${result.stderr}`, pattern);
}
function snapshot(directory) {
  if (!existsSync(directory)) return null;
  const entries = {};
  function walk(current, relative = '') {
    for (const entry of readdirSync(current)) {
      const native = path.join(current, entry), name = relative ? `${relative}/${entry}` : entry;
      const stat = lstatSync(native);
      if (stat.isSymbolicLink()) entries[name] = { link: readlinkSync(native) };
      else if (stat.isDirectory()) { entries[`${name}/`] = true; walk(native, name); }
      else entries[name] = readFileSync(native, 'utf8');
    }
  }
  walk(directory); return entries;
}
function seedLinux(f) {
  mkdirSync(path.join(f.prefix, 'bin'), { recursive: true });
  mkdirSync(path.join(f.prefix, 'share/applications'), { recursive: true });
  mkdirSync(path.join(f.prefix, 'lib/http-capture/releases/old/HTTP-Capture'), { recursive: true });
  writeFileSync(path.join(f.prefix, 'bin/http-capture'), 'OLD LAUNCHER\n');
  writeFileSync(path.join(f.prefix, 'share/applications/dev.httpcapture.desktop.desktop'), 'OLD DESKTOP\n');
  writeFileSync(path.join(f.prefix, 'lib/http-capture/releases/old/HTTP-Capture/AppRun'), 'OLD PAYLOAD\n');
}
function seedMac(f) {
  mkdirSync(path.join(f.prefix, 'HTTP Capture.app/Contents/MacOS'), { recursive: true });
  writeFileSync(path.join(f.prefix, 'HTTP Capture.app/Contents/MacOS/http-capture'), 'OLD MAC PAYLOAD\n');
}
// A small USTAR encoder makes unsafe path/link fixtures without relying on the
// host filesystem's symlink permissions. The installer still uses real tar.
function archive(entries) {
  const blocks = [];
  for (const { name, content = '', type = '0', link = '', mode = 0o755 } of entries) {
    const bytes = Buffer.from(content), header = Buffer.alloc(512);
    assert(Buffer.byteLength(name) <= 100); assert(Buffer.byteLength(link) <= 100);
    header.write(name, 0, 100); header.write(`${mode.toString(8).padStart(7, '0')}\0`, 100, 8);
    header.write('0000000\0', 108, 8); header.write('0000000\0', 116, 8);
    header.write(`${bytes.length.toString(8).padStart(11, '0')}\0`, 124, 12);
    header.write('00000000000\0', 136, 12); header.fill(32, 148, 156);
    header.write(type, 156, 1); header.write(link, 157, 100); header.write('ustar\0', 257, 6); header.write('00', 263, 2);
    const checksum = header.reduce((a, b) => a + b, 0);
    header.write(`${checksum.toString(8).padStart(6, '0')}\0 `, 148, 8);
    blocks.push(header, bytes, Buffer.alloc((512 - bytes.length % 512) % 512));
  }
  return gzipSync(Buffer.concat([...blocks, Buffer.alloc(1024)]));
}

test('installer syntax is valid Bash', () => {
  succeeds(shell(['-n', installer]));
});
test('Linux default installation stays under the fixture HOME and uses a real portable archive', () => {
  const f = fixture({ defaultPrefix: true }); normalPackage(f);
  succeeds(run(f, { latest: true }));
  const launcher = path.join(f.prefix, 'bin/http-capture');
  assert(existsSync(launcher));
  assert(existsSync(path.join(f.prefix, 'share/applications/dev.httpcapture.desktop.desktop')));
  assert.match(readFileSync(path.join(f.root, 'curl.log'), 'utf8'), /releases\/latest/);
  assert.match(readFileSync(path.join(f.root, 'curl.log'), 'utf8'), new RegExp(`download/${tag}/SHA256SUMS-linux-x64`));
  succeeds(shell([`${f.prefixShell}/bin/http-capture`, 'one argument', 'two'], { env: f.env }));
  assert.equal(readFileSync(path.join(f.root, 'launch.log'), 'utf8'), 'one argument\ntwo\n');
});
test('Linux explicit prefix with spaces survives a second install and retains the prior payload', () => {
  const f = fixture(); normalPackage(f, { version: 'first' }); succeeds(run(f));
  const releases = path.join(f.prefix, 'lib/http-capture/releases');
  const old = readdirSync(releases); assert.equal(old.length, 1);
  normalPackage(f, { version: 'second' }); succeeds(run(f));
  const current = readdirSync(releases); assert.equal(current.length, 2);
  assert.equal(readFileSync(path.join(releases, old[0], 'HTTP-Capture/fixture-version.txt'), 'utf8'), 'first');
  assert(current.some(name => readFileSync(path.join(releases, name, 'HTTP-Capture/fixture-version.txt'), 'utf8') === 'second'));
  const desktop = readFileSync(path.join(f.prefix, 'share/applications/dev.httpcapture.desktop.desktop'), 'utf8');
  assert(desktop.includes(`Exec="${f.prefixShell}/bin/http-capture"`));
  succeeds(shell([`${f.prefixShell}/bin/http-capture`, 'kept arguments'], { env: f.env }));
  assert.equal(readFileSync(path.join(f.root, 'launch.log'), 'utf8'), 'kept arguments\n');
});
for (const failure of ['hash', 'asset', 'checksums', 'duplicate-checksums', 'no-matching-package']) {
  test(`Linux ${failure} failure preserves the entire existing installation`, () => {
    const f = fixture(); normalPackage(f); seedLinux(f); const before = snapshot(f.prefix);
    const sums = path.join(f.root, `SHA256SUMS-${f.label}.txt`);
    if (failure === 'hash') packageBytes(f, readFileSync(path.join(f.root, 'package.tar.gz')), '0'.repeat(64));
    if (failure === 'duplicate-checksums') writeFileSync(sums, readFileSync(sums, 'utf8').repeat(2));
    if (failure === 'no-matching-package') writeFileSync(sums, `${'0'.repeat(64)}  unrelated.tar.gz\n`);
    const result = run(f, { env: { INSTALL_FIXTURE_FAIL: failure } });
    fails(result, failure === 'hash' ? /SHA256 mismatch/ : failure.includes('checksums') || failure === 'no-matching-package' ? /exactly one matching|Installing linux-x64/ : /Installing linux-x64/);
    assert(existsSync(path.join(f.root, 'curl.log')), 'failure must exercise the mock download path');
    assert.deepEqual(snapshot(f.prefix), before);
  });
}
for (const [description, entries] of [
  ['unexpected top-level directory', [{ name: 'Other/AppRun', content: '#!/bin/sh\n' }]],
  ['parent traversal', [{ name: 'HTTP-Capture/../../escaped', content: 'BAD' }]],
  ['absolute member', f => [{ name: `${f.shellRoot}/escaped`, content: 'BAD' }]],
]) {
  test(`Linux rejects archive with ${description} without changing the old installation`, () => {
    const f = fixture(); packageBytes(f, archive(typeof entries === 'function' ? entries(f) : entries)); seedLinux(f); const before = snapshot(f.prefix);
    fails(run(f)); assert.deepEqual(snapshot(f.prefix), before);
    assert(!existsSync(path.join(f.root, 'escaped')));
  });
}
test('Linux rejects a package without an executable AppRun and cleans the incomplete payload', { skip: process.platform === 'win32' ? 'MSYS does not preserve POSIX execute bits; native Linux CI runs this case.' : false }, () => {
  const f = fixture(); normalPackage(f, { executable: false }); seedLinux(f); const before = snapshot(f.prefix);
  fails(run(f)); assert.deepEqual(snapshot(f.prefix), before);
});
test('Linux rejects a package without AppRun and cleans the incomplete payload', () => {
  const f = fixture(); packageBytes(f, archive([{ name: 'HTTP-Capture/', type: '5' }, { name: 'HTTP-Capture/missing-entry.txt', content: 'fixture' }]));
  seedLinux(f); const before = snapshot(f.prefix);
  fails(run(f), /no executable AppRun/); assert.deepEqual(snapshot(f.prefix), before);
});
const posixLinkOptions = { skip: process.platform === 'win32' ? 'MSYS tar copies symlink targets; native Linux/macOS CI runs POSIX link cases.' : false };
test('Linux rejects an AppRun symlink outside the extracted portable tree', posixLinkOptions, () => {
  const f = fixture();
  packageBytes(f, archive([{ name: 'HTTP-Capture/', type: '5' }, { name: 'HTTP-Capture/AppRun', type: '2', link: '../../outside' }]));
  seedLinux(f); const before = snapshot(f.prefix);
  fails(run(f), /symlink/); assert.deepEqual(snapshot(f.prefix), before);
});
test('Linux retains ordinary relative library symlinks inside the portable tree', posixLinkOptions, () => {
  const f = fixture();
  packageBytes(f, archive([
    { name: 'HTTP-Capture/', type: '5' },
    { name: 'HTTP-Capture/AppRun', content: '#!/bin/sh\nexit 0\n' },
    { name: 'HTTP-Capture/lib/', type: '5' },
    { name: 'HTTP-Capture/lib/library.so.1', content: 'fixture library' },
    { name: 'HTTP-Capture/lib/library.so', type: '2', link: 'library.so.1' },
    { name: 'HTTP-Capture/usr/', type: '5' },
    { name: 'HTTP-Capture/usr/lib/', type: '5' },
    { name: 'HTTP-Capture/usr/lib/library.so', type: '2', link: '../../lib/library.so.1' },
  ]));
  succeeds(run(f));
  const releases = path.join(f.prefix, 'lib/http-capture/releases');
  const payload = path.join(releases, readdirSync(releases)[0], 'HTTP-Capture');
  assert.equal(readlinkSync(path.join(payload, 'lib/library.so')), 'library.so.1');
  assert.equal(readFileSync(path.join(payload, 'usr/lib/library.so'), 'utf8'), 'fixture library');
});
test('Linux rejects a library symlink which traverses above the application root', posixLinkOptions, () => {
  const f = fixture();
  packageBytes(f, archive([
    { name: 'HTTP-Capture/', type: '5' },
    { name: 'HTTP-Capture/AppRun', content: '#!/bin/sh\nexit 0\n' },
    { name: 'HTTP-Capture/lib/', type: '5' },
    { name: 'HTTP-Capture/lib/escape', type: '2', link: '../../outside' },
  ]));
  seedLinux(f); const before = snapshot(f.prefix);
  fails(run(f), /symlink/); assert.deepEqual(snapshot(f.prefix), before);
});
test('Linux rejects a library link that escapes via a chained symlink and parent traversal', posixLinkOptions, () => {
  const f = fixture();
  packageBytes(f, archive([
    { name: 'HTTP-Capture/', type: '5' },
    { name: 'HTTP-Capture/AppRun', content: '#!/bin/sh\nexit 0\n' },
    { name: 'HTTP-Capture/nested', type: '2', link: '.' },
    { name: 'HTTP-Capture/escape', type: '2', link: 'nested/../outside' },
  ]));
  seedLinux(f); const before = snapshot(f.prefix);
  fails(run(f), /symlink/); assert.deepEqual(snapshot(f.prefix), before);
});
for (const resolved of ['https://github.com/other/repo/releases/tag/' + tag, repo + '/releases/tag/not-a-build', repo + '/releases/tag/' + tag + '?unexpected=1']) {
  test(`latest redirect must resolve to a strict immutable tag in this repository: ${resolved}`, () => {
    const f = fixture(); normalPackage(f); seedLinux(f); const before = snapshot(f.prefix);
    fails(run(f, { latest: true, env: { INSTALL_FIXTURE_RESOLVED: resolved } }));
    assert.deepEqual(snapshot(f.prefix), before);
    assert.equal(readFileSync(path.join(f.root, 'curl.log'), 'utf8').trim().split('\n').length, 1);
  });
}
test('unsupported Linux ARM64 is rejected before downloads or installation writes', () => {
  const f = fixture({ arch: 'aarch64' });
  fails(run(f), /No published.*Linux.*aarch64/);
  assert(!existsSync(path.join(f.root, 'curl.log'))); assert(!existsSync(f.prefix));
});
for (const [description, options, label] of [
  ['native Apple Silicon', { os: 'Darwin', arch: 'arm64' }, 'macos-arm64'],
  ['Intel', { os: 'Darwin', arch: 'x86_64' }, 'macos-x64'],
  ['Rosetta Apple Silicon', { os: 'Darwin', arch: 'x86_64', arm64: '1' }, 'macos-arm64'],
]) {
  test(`macOS ${description} selects the matching archive and installs the full app`, () => {
    const f = fixture(options); normalPackage(f); succeeds(run(f));
    assert.equal(f.label, label);
    assert(existsSync(path.join(f.prefix, 'HTTP Capture.app/Contents/MacOS/http-capture')));
    assert.match(readFileSync(path.join(f.root, 'curl.log'), 'utf8'), new RegExp(`SHA256SUMS-${label}\\.txt`));
    assert.match(readFileSync(path.join(f.root, 'codesign.log'), 'utf8'), /--verify --deep --strict .*HTTP Capture\.app/);
  });
}
test('macOS default install uses fixture ~/Applications and retains the old app on replacement', () => {
  const f = fixture({ os: 'Darwin', arch: 'arm64', defaultPrefix: true }); normalPackage(f); seedMac(f);
  succeeds(run(f));
  const old = readdirSync(f.prefix).find(name => name.startsWith('.http-capture-previous.'));
  assert(old);
  assert.equal(readFileSync(path.join(f.prefix, old, 'HTTP Capture.app/Contents/MacOS/http-capture'), 'utf8'), 'OLD MAC PAYLOAD\n');
  assert(existsSync(path.join(f.prefix, 'HTTP Capture.app/fixture-version.txt')));
});
test('macOS codesign failure never replaces the installed app', () => {
  const f = fixture({ os: 'Darwin', arch: 'arm64' }); normalPackage(f); seedMac(f); const before = snapshot(f.prefix);
  fails(run(f, { env: { INSTALL_FIXTURE_CODESIGN_FAIL: '1' } }), /signature verification failed/);
  assert.deepEqual(snapshot(f.prefix), before);
});

test('macOS failed incoming app move restores the prior app bundle', () => {
  const f = fixture({ os: 'Darwin', arch: 'arm64' }); normalPackage(f); seedMac(f); const before = snapshot(f.prefix);
  fails(run(f, { env: { INSTALL_FIXTURE_MOVE_FAIL: '1' } }), /Fixture move failure/);
  assert.deepEqual(snapshot(f.prefix), before);
});
