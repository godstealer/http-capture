import { execFileSync } from 'node:child_process';
import { appendFileSync, copyFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const directory = path.join(root, '.local', 'custom-engine-fixture');
const executable = path.join(directory, `http-capture-custom${process.platform === 'win32' ? '.exe' : ''}`);
mkdirSync(directory, { recursive: true });
execFileSync('go', ['build', '-o', executable, path.join(root, 'examples/custom-send-engine/main.go')], { cwd: root, stdio: 'inherit' });
copyFileSync(path.join(root, 'examples/custom-send-engine/http-capture-custom.json'), path.join(directory, 'http-capture-custom.json'));
if (process.env.GITHUB_ENV) appendFileSync(process.env.GITHUB_ENV, `HTTP_CAPTURE_CUSTOM_ENGINE=${executable}\n`);
process.stdout.write(`${executable}\n`);
