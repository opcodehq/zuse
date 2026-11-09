#!/usr/bin/env node
// Prepare deterministic sidecars before Trunk publishes its staging directory.
import fs from 'node:fs';
import path from 'node:path';
import {gzipSync, brotliCompressSync, constants} from 'node:zlib';

const directory = process.argv[2] ?? process.env.TRUNK_STAGING_DIR;
if (!directory || !fs.statSync(directory).isDirectory()) throw new Error('Missing browser build directory');
const files = [];
function visit(directory) {
  for (const entry of fs.readdirSync(directory, {withFileTypes:true})) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) visit(file);
    else if (entry.isFile() && !/\.(gz|br)$/.test(entry.name) && entry.name !== 'network-budget.json') files.push(file);
  }
}
visit(directory);
const assets = [];
for (const file of files.sort()) {
  const raw = fs.readFileSync(file);
  const compressible = /\.(wasm|js|css|html|json|svg)$/.test(file);
  const encodings = {
    gzip: compressible ? gzipSync(raw, {level:9}) : raw,
    brotli: compressible ? brotliCompressSync(raw, {params:{[constants.BROTLI_PARAM_QUALITY]:11}}) : raw,
  };
  const asset = {path:path.relative(directory,file), rawBytes:raw.length};
  for (const [encoding, compressed] of Object.entries(encodings)) {
    const sidecar = file + (encoding === 'gzip' ? '.gz' : '.br');
    if (compressed.length < raw.length) fs.writeFileSync(sidecar, compressed);
    else fs.rmSync(sidecar, {force:true});
    asset[encoding + 'Bytes'] = Math.min(raw.length, compressed.length);
  }
  assets.push(asset);
  console.log(`${asset.path}: raw ${asset.rawBytes}, gzip ${asset.gzipBytes}, brotli ${asset.brotliBytes} bytes`);
}
const totalBrotliBytes = assets.reduce((sum, asset) => sum + asset.brotliBytes, 0);
const totalGzipBytes = assets.reduce((sum, asset) => sum + asset.gzipBytes, 0);
const budget = Number(process.env.ZUSE_WEB_MAX_BROTLI_BYTES ?? 5 * 1024 * 1024);
if (!Number.isFinite(budget) || budget <= 0) throw new Error('Invalid ZUSE_WEB_MAX_BROTLI_BYTES');
fs.writeFileSync(path.join(directory,'network-budget.json'), JSON.stringify({
  version:1, assets, totalBrotliBytes, totalGzipBytes,
  // Transfer-only estimate; excludes latency, compilation and first paint.
  secondsAt400Kbps:totalBrotliBytes / 50000,
  maxReleaseBrotliBytes:budget,
},null,2) + '\n');
if (process.env.TRUNK_PROFILE === 'release' && totalBrotliBytes > budget) {
  throw new Error(`Browser transfer budget exceeded: ${totalBrotliBytes} > ${budget} bytes. Inspect network-budget.json.`);
}
