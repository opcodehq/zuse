#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { homedir } from 'node:os';
import { join } from 'node:path';
const binary = process.env.ZUSE_CONTROL_BINARY || join(homedir(), '.zuse', 'bin', process.platform === 'win32' ? 'zuse-control.exe' : 'zuse-control');
const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });
child.on('error', () => { console.error('Zuse control binary is unavailable. Install or update Zuse, or set ZUSE_CONTROL_BINARY.'); process.exitCode = 1; });
child.on('exit', code => { process.exitCode = code ?? 1; });
