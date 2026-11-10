#!/usr/bin/env python3
"""Create a fresh portable bundle from verified immutable native inputs by hardlink."""
import argparse,hashlib,json,os,stat
from pathlib import Path

def canonical(value):return json.dumps(value,sort_keys=True,separators=(',',':')).encode()+b'\n'
def digest(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  while data:=f.read(1024*1024):h.update(data)
 return h.hexdigest()
def write_new(path,bytes):
 with path.open('xb') as f:os.fchmod(f.fileno(),0o444);f.write(bytes);f.flush();os.fsync(f.fileno())
def stage(source,installer,helper,output,source_digest,runtime_digest,installer_digest):
 if digest(source/'bundle.json')!=source_digest or digest(installer)!=installer_digest:raise ValueError('portable_input_pin_mismatch')
 original=json.loads((source/'bundle.json').read_text());files=original['files']
 if len(files)!=14 or len({v['path'] for v in files})!=14 or original['runtimeSha256']!=runtime_digest or any('/' in v['path'] or v['path'] in ('.','..') for v in files):raise ValueError('portable_bundle_shape_invalid')
 before={}
 for entry in files:
  path=source/entry['path'];info=path.lstat()
  if not stat.S_ISREG(info.st_mode) or info.st_size!=entry['bytes'] or digest(path)!=entry['sha256']:raise ValueError('portable_input_changed')
  before[entry['path']]=(info.st_dev,info.st_ino,info.st_mode,info.st_uid,info.st_gid,info.st_size)
 plan=json.loads((source/'prepare-plan.json').read_text())
 for key in ('buildId','templateVersion','configurationDigest'):plan.pop(key,None)
 seed=dict(version=1,parentBundleSha256=source_digest,installerSha256=installer_digest,portableHelperSha256=digest(helper),runtimeSha256=runtime_digest,harnessRelease=original['harnessRelease'],plan=plan)
 release='zuse-base-'+hashlib.sha256(canonical(seed)).hexdigest()[:32]
 plan.update(buildId=release,templateVersion=release,configurationDigest=release)
 output.mkdir(mode=0o700)
 linked=[]
 for entry in files:
  name=entry['path']
  if name in ('managed-base-install.sh','prepare-plan.json'):continue
  os.link(source/name,output/name);linked.append(name)
 # Never chmod a linked inode. Only these two newly allocated files are replaced.
 write_new(output/'managed-base-install.sh',installer.read_bytes());write_new(output/'prepare-plan.json',canonical(plan))
 manifest=dict(original,releaseId=release,files=[dict(path=entry['path'],bytes=(output/entry['path']).stat().st_size,sha256=digest(output/entry['path'])) for entry in files])
 write_new(output/'bundle.json',canonical(manifest))
 for name,identity in before.items():
  info=(source/name).stat()
  if (info.st_dev,info.st_ino,info.st_mode,info.st_uid,info.st_gid,info.st_size)!=identity:raise ValueError('portable_original_input_mutated')
 for name in linked:
  if not os.path.samefile(source/name,output/name):raise ValueError('portable_hardlink_proof_failed')
 return dict(version=1,status='staged-only',bundle=str(output),releaseId=release,bundleSha256=digest(output/'bundle.json'),runtimeSha256=runtime_digest,harnessRelease=manifest['harnessRelease'],installerSha256=installer_digest,helperSha256=digest(helper),hardlinkedFiles=linked,replacedFiles=['managed-base-install.sh','prepare-plan.json'],originalFileMetadataPreserved=True,logicalPayloadBytes=sum(f['bytes'] for f in manifest['files']),newPayloadBytes=(output/'managed-base-install.sh').stat().st_size+(output/'prepare-plan.json').stat().st_size+(output/'bundle.json').stat().st_size,providerEffectsPerformed=False)
def main():
 p=argparse.ArgumentParser(description=__doc__)
 for key in ('source','installer','helper','output','source-sha256','runtime-sha256','installer-sha256'):p.add_argument('--'+key,required=True)
 a=p.parse_args();print(json.dumps(stage(Path(a.source),Path(a.installer),Path(a.helper),Path(a.output),a.source_sha256,a.runtime_sha256,a.installer_sha256),sort_keys=True))
if __name__=='__main__':main()
