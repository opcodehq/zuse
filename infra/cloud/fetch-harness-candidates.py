#!/usr/bin/env python3
"""Download exact native npm distributions with registry signature/SRI verification.

Produces candidates only. No channel promotion, account login or provider allocation.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.parse
import urllib.request

MAX_ARCHIVE = 512 * 1024 * 1024
VERSION = re.compile(r'\d+\.\d+\.\d+(?:[-+][a-zA-Z0-9.-]+)?\Z')


def get(url, limit=1024*1024):
    if urllib.parse.urlparse(url).netloc != 'registry.npmjs.org':
        raise ValueError('npm_registry_origin_required')
    with urllib.request.urlopen(url, timeout=30) as response:
        if urllib.parse.urlparse(response.url).netloc != 'registry.npmjs.org':
            raise ValueError('npm_registry_redirect_rejected')
        data=response.read(limit+1)
    if len(data)>limit: raise ValueError('registry_response_too_large')
    return data


def verify_signature(metadata, keys):
    message=f"{metadata['name']}@{metadata['version']}:{metadata['dist']['integrity']}".encode()
    for signature in metadata['dist'].get('signatures', []):
        key=next((key for key in keys if key['keyid']==signature['keyid']),None)
        if key is None: continue
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            (root/'key.der').write_bytes(base64.b64decode(key['key'],validate=True))
            (root/'signature').write_bytes(base64.b64decode(signature['sig'],validate=True))
            subprocess.run(['openssl','pkey','-pubin','-inform','DER','-in',str(root/'key.der'),'-out',str(root/'key.pem')],
                           check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            checked=subprocess.run(['openssl','dgst','-sha256','-verify',str(root/'key.pem'),'-signature',str(root/'signature')],
                                   input=message,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            if checked.returncode==0: return key['keyid']
    raise ValueError('npm_distribution_signature_unverified')


def codex_assets(tar, members, output, executable):
    prefix=executable.rsplit('/bin/codex',1)[0]+'/'
    assets={};target=output/'codex-assets'
    for member in members:
        path=Path(member.name)
        if path.is_absolute() or '..' in path.parts or member.issym() or member.islnk():
            raise ValueError('unsafe_distribution_member')
        if not member.isfile() or member.name==executable:continue
        if member.name.startswith(prefix):relative='codex-package/'+member.name[len(prefix):]
        elif member.name in ('package/package.json','package/README.md'):
            relative='codex-package/'+('npm-package.json' if member.name.endswith('package.json') else 'README.md')
        else:continue
        if relative in assets or member.size>128*1024*1024:raise ValueError('invalid_distribution_asset')
        destination=target/relative;destination.parent.mkdir(parents=True,exist_ok=True)
        with tar.extractfile(member) as source,destination.open('xb') as dest:
            shutil.copyfileobj(source,dest,1024*1024)
        executable_asset='/bin/' in relative or relative.endswith('/bwrap') or '/codex-path/' in relative
        destination.chmod(0o555 if executable_asset else 0o444)
        assets[relative]={'sha256':hashlib.sha256(destination.read_bytes()).hexdigest(),'bytes':member.size,'executable':executable_asset}
    required={'codex-package/bin/codex-code-mode-host','codex-package/codex-resources/bwrap','codex-package/codex-path/rg','codex-package/codex-package.json','codex-package/npm-package.json','codex-package/README.md'}
    if not required.issubset(assets):raise ValueError('codex_distribution_incomplete')
    (target/'assets.json').write_text(json.dumps(assets,sort_keys=True)+'\n')
    return assets


def fetch(agent, version, platform, output, keys, node=None, previous=None):
    package=({'claude':'@anthropic-ai/claude-code-'+platform,'grok':'@xai-official/grok-'+platform,
              'codex':'@openai/codex'})[agent]
    npm_version=version+'-'+platform if agent=='codex' else version
    metadata=json.loads(get('https://registry.npmjs.org/'+urllib.parse.quote(package,safe='')+'/'+npm_version))
    if metadata['name']!=package or metadata['version']!=npm_version: raise ValueError('package_identity_mismatch')
    signing_key=verify_signature(metadata,keys)
    integrity=metadata['dist']['integrity']
    if not integrity.startswith('sha512-'): raise ValueError('sha512_integrity_required')
    url=metadata['dist']['tarball']
    if urllib.parse.urlparse(url).netloc!='registry.npmjs.org':raise ValueError('tarball_origin_rejected')
    destination=output/agent
    if previous and previous.get('version')==version and previous.get('npmIntegrity')==integrity and destination.is_file():
        digest=hashlib.sha256()
        with destination.open('rb') as binary:
            while block:=binary.read(1024*1024):digest.update(block)
        if digest.hexdigest()!=previous.get('sha256'):raise ValueError('cached_candidate_integrity_mismatch')
        if agent!='codex' or previous.get('assets'):
            if agent=='codex':
                for name,pin in previous['assets'].items():
                    path=output/'codex-assets'/name
                    if path.is_symlink() or not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest()!=pin['sha256']:raise ValueError('cached_distribution_asset_mismatch')
            return previous
        destination.unlink()
    with tempfile.TemporaryFile() as archive:
        digest=hashlib.sha512();size=0
        with urllib.request.urlopen(url,timeout=60) as response:
            if urllib.parse.urlparse(response.url).netloc!='registry.npmjs.org':raise ValueError('tarball_redirect_rejected')
            while block:=response.read(1024*1024):
                size+=len(block)
                if size>MAX_ARCHIVE:raise ValueError('archive_limit_exceeded')
                digest.update(block);archive.write(block)
        if base64.b64encode(digest.digest()).decode()!=integrity[7:]:raise ValueError('tarball_integrity_mismatch')
        archive.seek(0)
        with tarfile.open(fileobj=archive,mode='r:gz') as tar:
            members=tar.getmembers()
            if len(members)>1024 or sum(m.size for m in members)>1024*1024*1024:raise ValueError('unpacked_limit_exceeded')
            selected=[m for m in members if m.isfile() and Path(m.name).name in ((agent,agent+'.br') if agent=='grok' else (agent,))]
            if len(selected)!=1:raise ValueError('standalone_native_executable_ambiguous')
            member=selected[0]
            assets=codex_assets(tar,members,output,member.name) if agent=='codex' else None
            if member.size>512*1024*1024:raise ValueError('native_binary_too_large')
            compressed=member.name.endswith('.br')
            selected_path=output/(agent+'.br') if compressed else destination
            with tar.extractfile(member) as source,selected_path.open('xb') as target:
                while block:=source.read(1024*1024):target.write(block)
            if compressed:
                if not node:raise ValueError('node_required_for_vendor_brotli_binary')
                script="const fs=require('fs'),z=require('zlib'),s=require('stream');let n=0;const limit=new s.Transform({transform(c,e,cb){n+=c.length;cb(n>536870912?new Error('limit'):null,c)}});s.pipeline(fs.createReadStream(process.argv[1]),z.createBrotliDecompress(),limit,fs.createWriteStream(process.argv[2],{flags:'wx'}),e=>{if(e)process.exitCode=1});"
                subprocess.run([str(node),'-e',script,str(selected_path),str(destination)],check=True,timeout=60,
                               env={'PATH':'/usr/bin:/bin'},stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
                selected_path.unlink()
            with destination.open('rb') as binary:
                if binary.read(4)!=b'\x7fELF':raise ValueError('native_linux_elf_required')
            destination.chmod(0o555)
    with tempfile.TemporaryDirectory() as home:
        result=subprocess.run([str(destination),'--version'],capture_output=True,timeout=30,check=True,
            env={'PATH':'/usr/bin:/bin','HOME':home,'DISABLE_AUTOUPDATER':'1','DISABLE_UPDATES':'1'})
    if not re.search(r'(?<![\w.])'+re.escape(version)+r'(?![\w.+-])',result.stdout.decode()):raise ValueError('native_version_mismatch')
    executable_digest=hashlib.sha256()
    with destination.open('rb') as binary:
        while block:=binary.read(1024*1024):executable_digest.update(block)
    return dict(agent=agent,version=version,package=package,npmVersion=npm_version,npmIntegrity=integrity,
                registrySigningKey=signing_key,sha256=executable_digest.hexdigest(),
                executableMember=member.name,archiveBytes=size,status='candidate-only',**({'assets':assets,'executable':'codex-package/bin/codex'} if assets else {}))


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for agent in ('claude','codex','grok'):parser.add_argument('--'+agent+'-version',required=True)
    parser.add_argument('--platform',choices=('linux-x64','linux-arm64'),default='linux-x64')
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--node',type=Path,help='trusted Node executable for vendor Grok brotli decompression')
    parser.add_argument('--resume',action='store_true')
    args=parser.parse_args()
    if any(not VERSION.fullmatch(getattr(args,agent+'_version')) for agent in ('claude','codex','grok')):parser.error('exact versions required')
    if args.output.exists() and not args.resume:parser.error('output must be a new directory or explicit --resume')
    args.output.mkdir(mode=0o700,parents=True,exist_ok=args.resume)
    previous=json.loads((args.output/'provenance.json').read_text()) if args.resume else {'harnesses':[]}
    report={'schema':1,'status':'incomplete','platform':args.platform,'harnesses':[]}
    try:
        keys=json.loads(get('https://registry.npmjs.org/-/npm/v1/keys'))['keys']
        for agent in ('claude','codex','grok'):
            report['harnesses'].append(fetch(agent,getattr(args,agent+'_version'),args.platform,args.output,keys,args.node,
                next((entry for entry in previous['harnesses'] if entry['agent']==agent),None)))
            (args.output/'provenance.json').write_text(json.dumps(report,indent=2)+'\n')
        report['status']='candidate-only'
    finally:
        path=args.output/'provenance.json';path.write_text(json.dumps(report,indent=2)+'\n');path.chmod(0o600)
    print(json.dumps({'status':report['status'],'platform':args.platform,'agents':[r['agent'] for r in report['harnesses']]}))


if __name__=='__main__':main()
