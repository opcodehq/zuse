#!/usr/bin/env node
// Converts only the mapped icon data; no renderer or npm install scripts run.
import {readFile, writeFile, mkdir, mkdtemp, rm} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {tmpdir} from 'node:os';
import {resolve, join, dirname} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const argument = name => { const i = args.indexOf(name); return i < 0 ? undefined : args[i + 1]; };
const output = argument('--output');
if (!output) throw new Error('Specify --output for generated icons');
const key = process.env.HUGEICONS_LICENSE_KEY?.trim();
const name = key ? '@hugeicons-pro/core-solid-rounded' : '@hugeicons/core-free-icons';
let temporary;
try {
  const mapText = await readFile(join(root, 'crates/icons/mapping.tsv'), 'utf8');
  const mapping = createHash('sha256').update(mapText).digest('hex');
  const authorization = key ? createHash('sha256').update(key).digest('hex') : undefined;
  const rows = mapText.trim().split('\n').map(line => line.split('\t'));
  if (key && args.includes('--reuse')) {
    let cached;
    try { cached = JSON.parse(await readFile(join(output, 'provenance.json'), 'utf8')); } catch {}
    if (cached?.package === name && cached.mapping === mapping && cached.authorization === authorization && (!process.env.HUGEICONS_SOLID_VERSION || cached.version === process.env.HUGEICONS_SOLID_VERSION)) {
      const files = await Promise.all(rows.map(async ([name]) => { try { return (await readFile(join(output, `${name}.svg`), 'utf8')).startsWith('<svg '); } catch { return false; } }));
      if (files.every(Boolean)) { console.log(`Using ${rows.length} prepared solid Hugeicons`); process.exit(0); }
    }
  }
  let packageRoot = argument('--package');
  let provenance;
  if (!packageRoot) {
    temporary = await mkdtemp(join(tmpdir(), 'zuse-icons-'));
    const registry = new URL(key ? 'https://npm.hugeicons.com/' : 'https://registry.npmjs.org/');
    const headers = key ? {Authorization: `Bearer ${key}`} : {};
    const metadata = await fetch(new URL(name.replace('/', '%2f'), registry), {headers, redirect: 'error', signal: AbortSignal.timeout(30000)});
    if (!metadata.ok) throw new Error(`Icon registry returned ${metadata.status}`);
    const info = await metadata.json();
    const version = key ? (process.env.HUGEICONS_SOLID_VERSION || info['dist-tags']?.latest) : '4.2.1';
    const entry = info.versions?.[version];
    if (!entry?.dist?.integrity?.startsWith('sha512-')) throw new Error('Icon package lacks a SHA-512 integrity record');
    const tarball = new URL(entry.dist.tarball);
    if (tarball.protocol !== 'https:') throw new Error('Icon package must use HTTPS');
    let current = tarball;
    let response;
    for (let attempt = 0; attempt < 6; attempt++) {
      if (current.protocol !== 'https:') throw new Error('Icon redirects must use HTTPS');
      response = await fetch(current, {headers: current.origin === registry.origin ? headers : {}, redirect: 'manual', signal: AbortSignal.timeout(60000)});
      if (![301, 302, 303, 307, 308].includes(response.status)) break;
      const location = response.headers.get('location');
      if (!location) throw new Error('Icon redirect is missing its destination');
      await response.body?.cancel();
      current = new URL(location, current);
    }
    if (!response.ok) throw new Error(`Icon download returned ${response.status}`);
    const bytes = Buffer.from(await response.arrayBuffer());
    if (`sha512-${createHash('sha512').update(bytes).digest('base64')}` !== entry.dist.integrity) throw new Error('Icon package integrity mismatch');
    const archive = join(temporary, 'icons.tgz'); await writeFile(archive, bytes, {mode: 0o600});
    const listing = spawnSync('tar', ['-tzf', archive], {encoding: 'utf8'});
    if (listing.status !== 0 || listing.stdout.split('\n').filter(Boolean).some(path => !path.startsWith('package/') || path.split('/').includes('..'))) throw new Error('Invalid icon archive');
    const unpacked = spawnSync('tar', ['-xzf', archive, '-C', temporary], {stdio: 'ignore'});
    if (unpacked.status !== 0) throw new Error('Could not unpack icon package');
    packageRoot = join(temporary, 'package');
    provenance = {package: name, version, integrity: entry.dist.integrity};
  } else {
    const info = JSON.parse(await readFile(join(packageRoot, 'package.json'), 'utf8'));
    if (info.name !== name) throw new Error('Prepared package does not match the selected icon style');
    provenance = {package: info.name, version: info.version};
  }
  const escape = value => String(value).replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  const attribute = name => name === 'className' ? 'class' : name.replace(/[A-Z]/g, letter => `-${letter.toLowerCase()}`);
  const render = ([tag, attrs]) => {
    if (!['path', 'circle', 'rect', 'ellipse', 'line', 'polyline', 'polygon'].includes(tag)) throw new Error('Unsupported icon element');
    return `<${tag} ${Object.entries(attrs).filter(([name]) => name !== 'key').map(([name, value]) => `${attribute(name)}="${escape(value)}"`).join(' ')}/>`;
  };
  // Resolve all exports before writing, so a missing solid glyph fails the build.
  const svgs = await Promise.all(rows.map(async ([name, exported]) => {
    const data = (await import(pathToFileURL(resolve(packageRoot, 'dist/esm', `${exported}.js`)))).default;
    if (!Array.isArray(data)) throw new Error('Invalid icon data');
    return [name, `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none">${data.map(render).join('')}</svg>\n`];
  }));
  await mkdir(output, {recursive: true});
  for (const [name, svg] of svgs) await writeFile(join(output, `${name}.svg`), svg);
  await writeFile(join(output, 'provenance.json'), `${JSON.stringify(key ? {...provenance, mapping, authorization} : provenance, null, 2)}\n`);
  console.log(`Prepared ${svgs.length} ${key ? 'solid' : 'free'} Hugeicons`);
} catch (error) {
  // Never echo registry URLs, request headers or package-manager config.
  console.error(error instanceof Error && !key ? error.message : 'Unable to prepare licensed solid icons');
  process.exitCode = 1;
} finally { if (temporary) await rm(temporary, {recursive: true, force: true}); }
