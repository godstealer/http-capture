import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = fileURLToPath(new URL('../', import.meta.url));
const local = process.platform === 'win32' && existsSync(path.join(root, '.local/cargo/bin/cargo.exe'));
const child = spawn(local ? 'powershell.exe' : 'cargo', local
  ? ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(root, 'scripts/rust.ps1'), ...process.argv.slice(2)]
  : process.argv.slice(2), { cwd: root, stdio: 'inherit', windowsHide: true });
child.on('error', error => { console.error(error.message); process.exitCode = 1; });
child.on('exit', code => { process.exitCode = code ?? 1; });
