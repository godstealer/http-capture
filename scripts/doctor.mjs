import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
console.log(`Node: ${process.version}`);
console.log(`Project-local Rust: ${existsSync('.local/cargo/bin/rustc.exe') ? 'installed' : 'not installed'}`);
const result = spawnSync('rustc', ['--version'], { encoding: 'utf8', windowsHide: true });
console.log(`System Rust: ${result.stdout?.trim() || 'not on PATH'}`);
console.log('GUI: npm run dev\nCapture service: npm run capture\nNative desktop: npm run desktop (requires platform toolchain)');
