import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync, readFileSync, copyFileSync, existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const staging = path.join(root, 'apps/desktop/src-tauri/release-resources');
mkdirSync(staging, { recursive: true });
const executable = path.join(staging, `http-capture-httpcloak${process.platform === 'win32' ? '.exe' : ''}`);
const helper = path.join(root, 'helpers/httpcloak');
execFileSync('go', ['build', '-mod=readonly', '-trimpath', '-o', executable, '.'], { cwd: helper, stdio: 'inherit' });
const capabilities = execFileSync(executable, ['--capabilities'], { encoding: 'utf8' });
const parsed = JSON.parse(capabilities);
if (parsed.protocolVersion !== 1 || !parsed.browserVersions?.chrome?.length) throw new Error('Invalid packaged helper capabilities');
writeFileSync(path.join(staging, 'http-capture-httpcloak.json'), capabilities);
// Include the exact Go dependency license files in the application resources.
const modules = execFileSync('go', ['list', '-m', '-mod=readonly', '-f', '{{.Path}}|{{.Dir}}', 'all'], { cwd: helper, encoding: 'utf8' });
const notices = [];
for (const line of modules.trim().split(/\r?\n/)) {
  const [name, dir] = line.split('|');
  if (!dir || name === 'http-capture/helpers/httpcloak') continue;
  const license = ['LICENSE', 'LICENSE.txt', 'LICENSE.md', 'COPYING'].map(f => path.join(dir, f)).find(existsSync);
  if (license) notices.push(`\n===== ${name} =====\n${readFileSync(license, 'utf8')}`);
}
if (!notices.length) throw new Error('No Go dependency licenses found');
writeFileSync(path.join(staging, 'GO-THIRD-PARTY-LICENSES.txt'), notices.join('\n'));
copyFileSync(path.join(root, 'vendor/h2/LICENSE'), path.join(staging, 'H2-LICENSE.txt'));
writeFileSync(path.join(root, 'apps/desktop/src-tauri/tauri.release.json'), JSON.stringify({
  bundle: { resources: { 'release-resources/': 'helpers/' }, icon: ['icons/32x32.png', 'icons/128x128.png', 'icons/128x128@2x.png', 'icons/icon.icns', 'icons/icon.ico'] },
}, null, 2));
