#!/usr/bin/env python3
"""Plan or explicitly approved private legacy snapshot. Never writes source DB.

Default plan returns catalog/count/size metadata only. Export requires the exact
source fingerprint from the reviewed plan and a new private directory outside the
checkout. No connection URL, customer row or encryption key is printed.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import urllib.parse

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('legacy_rehearsal',HERE/'migration-rehearsal.py')
rehearsal=importlib.util.module_from_spec(spec);spec.loader.exec_module(rehearsal)

def private_source(path):
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
 try:
  st=os.fstat(fd)
  if not stat.S_ISREG(st.st_mode) or st.st_uid not in (0,os.getuid()) or st.st_mode&0o077 or st.st_size>16384:raise ValueError('zuse_rehearsal_private_source_file_required')
  return os.read(fd,16385).decode().strip()
 finally:os.close(fd)

def describe(url,pg_dump='pg_dump'):
 env=rehearsal.database.connection_environment(url)
 source=hashlib.sha256(json.dumps({k:env[k] for k in ('PGHOST','PGPORT','PGDATABASE','PGUSER')},sort_keys=True).encode()).hexdigest()
 expected=rehearsal.table_inventory()
 sql="""BEGIN READ ONLY;SET LOCAL statement_timeout='15s'; SELECT json_build_object('serverVersionNumber',current_setting('server_version_num'),'tables',coalesce(jsonb_agg(c.relname ORDER BY c.relname),'[]'::jsonb),'tableAndToastBytes',coalesce(sum(pg_table_size(c.oid)),0),'indexBytes',coalesce(sum(pg_indexes_size(c.oid)),0),'approximateRows',coalesce(sum(c.reltuples::bigint),0)) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname LIKE 'api_%' AND c.relkind='r';COMMIT;"""
 metadata=rehearsal.execute(url,sql)[-1]
 if set(metadata['tables'])!=set(expected['retained'])|set(expected['quarantined']):raise ValueError('zuse_rehearsal_unclassified_source_table')
 version=subprocess.run([pg_dump,'--version'],capture_output=True,text=True,timeout=5,check=True)
 match=re.search(r'PostgreSQL\) (\d+)',version.stdout)
 if not match:raise ValueError('zuse_rehearsal_dump_version_unknown')
 return dict(mode='read-only-export-plan',sourceReference=rehearsal.REFERENCE,sourceFingerprint=source,
             sourceSchema='public',sourceTableCount=len(metadata['tables']),retainedTableCount=len(expected['retained']),
             quarantineTableCount=len(expected['quarantined']),sourceMetadata=metadata,
             pgDumpMajor=int(match[1]),requiredPgDumpMajor=int(metadata['serverVersionNumber'])//10000,
             sourceWritesAllowed=False,writerCutoverAllowed=False,customerPayloadExported=False,
             sourceObjectsCopied=False,encryptionKeyMatchVerified=False,
             exportScope='public.api_* tables plus owned sequences; no other schemas or object payloads')

def export(url,directory,approval,pg_dump='pg_dump'):
 plan=describe(url,pg_dump)
 if approval!=plan['sourceFingerprint']:raise ValueError('zuse_rehearsal_exact_source_approval_required')
 if plan['pgDumpMajor']<plan['requiredPgDumpMajor']:raise ValueError('zuse_rehearsal_matching_pg_dump_required')
 root=Path(directory).absolute()
 checkout=HERE.parents[1]
 if root.resolve().is_relative_to(checkout) or root.exists() or root.is_symlink():raise ValueError('zuse_rehearsal_new_private_export_directory_required')
 root.mkdir(mode=0o700,parents=False)
 destination=root/'legacy-public.dump'
 fd=os.open(destination,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 env=rehearsal.database.connection_environment(url)
 # pg_dump uses a consistent read-only snapshot, including data and sequence
 # high-water marks. Old production writers continue; no handoff is performed.
 env['PGOPTIONS']='-c default_transaction_read_only=on -c statement_timeout=0'
 command=[pg_dump,'--no-password','--format=custom','--compress=zstd:3','--no-owner','--no-acl',
          '--no-comments','--strict-names','--table=public.api_*',
          '--serializable-deferrable','--lock-wait-timeout=5s']
 try:
  with os.fdopen(fd,'wb') as output:
   result=subprocess.run(command,stdout=output,stderr=subprocess.PIPE,env=env,timeout=600)
   output.flush();os.fsync(output.fileno())
  if result.returncode:raise ValueError('zuse_rehearsal_private_export_failed')
  digest=hashlib.sha256()
  with destination.open('rb') as source:
   for chunk in iter(lambda:source.read(1024*1024),b''):digest.update(chunk)
  # Keep data private. Only metadata/checksum go into the companion manifest.
  manifest=dict(plan,mode='private-consistent-export',customerPayloadExported=True,
                sourceWritesAllowed=False,writerCutoverAllowed=False,
                bytes=destination.stat().st_size,sha256=digest.hexdigest(),file=destination.name,
                sourceWriter='legacy',targetWriter='none',restoreAllowedOnlyInOwnedIsolatedCluster=True)
  out=root/'manifest.json';descriptor=os.open(out,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
  with os.fdopen(descriptor,'w') as stream:json.dump(manifest,stream,sort_keys=True,indent=2);stream.flush();os.fsync(stream.fileno())
  return manifest
 except BaseException:
  # Preserve an uncertain partial export privately; never mistake it for a
  # complete manifest or overwrite it during an operator retry.
  raise

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('action',choices=('plan','export'))
 group=p.add_mutually_exclusive_group(required=True);group.add_argument('--source-url-file',type=Path);group.add_argument('--source-env',choices=('DATABASE_URL','ZUSE_LEGACY_DATABASE_URL'))
 p.add_argument('--output-directory',type=Path);p.add_argument('--approve-source-fingerprint');p.add_argument('--pg-dump',default='pg_dump');a=p.parse_args()
 url=private_source(a.source_url_file) if a.source_url_file else os.environ.get(a.source_env)
 if not url:raise ValueError('zuse_rehearsal_source_connection_required')
 if a.action=='plan':result=describe(url,a.pg_dump)
 else:
  if not a.output_directory:raise ValueError('zuse_rehearsal_private_export_directory_required')
  result=export(url,a.output_directory,a.approve_source_fingerprint,a.pg_dump)
 print(json.dumps(result,sort_keys=True))

if __name__=='__main__':
 try:main()
 except Exception as error:
  code=str(error) if re.fullmatch(r'zuse_(rehearsal|schema)_[a-z_]+',str(error)) else 'zuse_rehearsal_export_failed'
  print(json.dumps({'ok':False,'code':code,'customerRecordsPrinted':False}));raise SystemExit(1)
