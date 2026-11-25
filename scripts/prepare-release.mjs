import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync, readFileSync, copyFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readJson, releaseConfiguration } from './release-updater.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
// Validate release identity and the committed trust key before doing a costly build.
// This overlay intentionally leaves plugins.updater in tauri.conf.json untouched.
const configuration = releaseConfiguration(readJson(path.join(root, 'apps/desktop/src-tauri/tauri.conf.json')), process.env);
const staging = path.join(root, 'apps/desktop/src-tauri/release-resources');
mkdirSync(staging, { recursive: true });

function buildHelper(name) {
  const cwd = path.join(root, 'helpers', name);
  const filename = 'http-capture-' + name;
  const executable = path.join(staging, filename + (process.platform === 'win32' ? '.exe' : ''));
  execFileSync('go', ['build', '-mod=readonly', '-trimpath', '-o', executable, '.'], { cwd, stdio: 'inherit' });
  return { cwd, executable, manifest: filename + '.json' };
}

const httpcloak = buildHelper('httpcloak');
const capabilities = execFileSync(httpcloak.executable, ['--capabilities'], { encoding: 'utf8' });
const parsed = JSON.parse(capabilities);
if (parsed.protocolVersion !== 1 || !parsed.browserVersions?.chrome?.length) throw new Error('Invalid packaged helper capabilities');
writeFileSync(path.join(staging, httpcloak.manifest), capabilities);

const xhttp = buildHelper('xhttp');
const xhttpManifest = path.join(xhttp.cwd, xhttp.manifest);
assert.deepEqual(readJson(xhttpManifest), {
  protocolVersion: 1,
  profiles: ['native'],
  browserVersions: {},
  customClientHello: false,
  certificateRoots: true,
  clientCertificate: true,
}, 'Invalid xhttp helper capabilities');
copyFileSync(xhttpManifest, path.join(staging, xhttp.manifest));

// Sign the nested Mach-O before the application signature seals the resources.
const identity = process.env.APPLE_SIGNING_IDENTITY?.trim();
if (process.platform === 'darwin' && identity) {
  execFileSync('codesign', ['--force', '--sign', identity, xhttp.executable], { stdio: 'inherit' });
}

// Use the actual build dependency graph (rather than every module in go.mod),
// so the notices match linked modules and work with offline build caches.
const modules = new Map();
for (const helper of [httpcloak, xhttp]) {
  const { cwd } = helper;
  const deps = execFileSync('go', [
    'list', '-deps', '-mod=readonly', '-f',
    '{{with .Module}}{{if not .Main}}{{.Path}}@{{.Version}}|{{.Dir}}{{end}}{{end}}', '.',
  ], { cwd, encoding: 'utf8' });
  for (const line of deps.split(/\r?\n/).filter(Boolean)) {
    const [name, dir] = line.split('|');
    if (!dir) throw new Error('Go module directory unavailable: ' + name);
    modules.set(name, { dir, required: helper === xhttp });
  }
}
const goRoot = execFileSync('go', ['env', 'GOROOT'], { cwd: xhttp.cwd, encoding: 'utf8' }).trim();
modules.set('Go standard library', { dir: goRoot, required: true });
const notices = [];
for (const [name, { dir, required }] of [...modules].sort(([a], [b]) => a.localeCompare(b))) {
  const licenseNames = ['LICENSE', 'LICENSE.txt', 'LICENSE.md', 'COPYING'];
  const licenses = licenseNames.filter(file => existsSync(path.join(dir, file)));
  // Keep compatibility with existing httpcloak modules whose distributions
  // omit a root license. Every linked xhttp module must ship its license.
  if (!licenses.length && required) throw new Error('Go dependency license unavailable: ' + name);
  for (const file of [...licenses, 'NOTICE', 'NOTICE.txt', 'PATENTS']) {
    const filename = path.join(dir, file);
    if (existsSync(filename)) {
      notices.push('\n===== ' + name + ' / ' + file + ' =====\n' + readFileSync(filename, 'utf8'));
    }
  }
}
writeFileSync(path.join(staging, 'GO-THIRD-PARTY-LICENSES.txt'), notices.join('\n'));
copyFileSync(path.join(root, 'vendor/h2/LICENSE'), path.join(staging, 'H2-LICENSE.txt'));
writeFileSync(path.join(root, 'apps/desktop/src-tauri/tauri.release.json'), JSON.stringify(configuration, null, 2) + '\n');
