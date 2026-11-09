#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { readFile, mkdir, copyFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';

const [binary, output] = process.argv.slice(2);
if (!binary || !output) throw new Error('Usage: node scripts/package-control.mjs <binary> <bundle-directory>');
const pkg = JSON.parse(await readFile(new URL('../packages/sdk/package.json', import.meta.url), 'utf8'));
if (process.env.ZUSE_PACKAGE_CROSS !== '1') {
  const probe = execFileSync(resolve(binary), ['--version'], { encoding: 'utf8' }).trim();
  if (probe !== `zuse-control ${pkg.version}`) throw new Error('SDK and binary versions differ');
}
const packed = JSON.parse(execFileSync('npm', ['pack', '--json'], { cwd: new URL('../packages/sdk', import.meta.url), encoding: 'utf8' }));
const sdk = new URL(`../packages/sdk/${packed[0].filename}`, import.meta.url);
await mkdir(output, { recursive: true });
const name = process.platform === 'win32' ? 'zuse-control.exe' : 'zuse-control';
await copyFile(binary, join(output, name));
await copyFile(sdk, join(output, 'sdk.tgz'));
const checksum = async path => createHash('sha256').update(await readFile(path)).digest('hex');
await writeFile(join(output, 'manifest.json'), JSON.stringify({
  version: pkg.version, apiVersion: 1,
  binarySha256: await checksum(binary), sdkSha256: await checksum(sdk),
}, null, 2) + '\n');
console.log(`Prepared ${output} for Zuse ${pkg.version}`);
