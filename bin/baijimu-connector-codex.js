#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
const root = dirname(fileURLToPath(import.meta.url));
const platform = process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : process.platform;
const arch = process.arch === 'x64' ? 'x86_64' : process.arch;
const name = 'baijimu-connector-codex' + (process.platform === 'win32' ? '.exe' : '');
const candidates = [join(root, `${platform}-${arch}`, name)];
if (platform === 'macos') candidates.unshift(join(root, 'macos', name));
const executable = candidates.find(existsSync);
if (!executable) throw new Error('Install the signed native Connector package for this platform. No JavaScript runtime fallback is provided.');
const child = spawn(executable, process.argv.slice(2), { stdio: 'inherit' });
child.on('error', error => { console.error(error.message); process.exitCode = 1; });
child.on('exit', code => { process.exitCode = code ?? 1; });
