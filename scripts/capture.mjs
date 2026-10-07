import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = fileURLToPath(new URL('../', import.meta.url));
const args = ['run', '-p', 'capture-service', '--no-default-features', '--bin', 'capture-service'];
if (process.argv.includes('--browser-replay')) args.push('--features', 'browser-replay');
const local = path.join(root, '.local/cargo/bin/cargo.exe');
const command = process.platform === 'win32' && existsSync(local) ? 'powershell.exe' : 'cargo';
const commandArgs = command === 'powershell.exe'
  ? ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', path.join(root, 'scripts/rust.ps1'), ...args]
  : args;
const child = spawn(command, commandArgs, { cwd: root, stdio: 'inherit', windowsHide: true });
child.on('error', error => { console.error(`无法启动 Rust 内核：${error.message}`); process.exitCode = 1; });
child.on('exit', code => { process.exitCode = code ?? 1; });
