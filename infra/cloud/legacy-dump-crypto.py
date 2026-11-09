#!/usr/bin/env python3
"""Counts-only key compatibility check directly from the approved private dump.

No database/container/source connection is opened. Selected encrypted COPY rows
stay in private process memory; decrypted credentials never leave the proof child.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import subprocess

sp=importlib.util.spec_from_file_location('retained',Path(__file__).with_name('retained-rehearsal.py'))
retained=importlib.util.module_from_spec(sp);sp.loader.exec_module(retained)

def unescape(value):
 if value==r'\N':return None
 def one(match):
  text=match.group(1)
  if text[0] in '01234567':return chr(int(text,8))
  if text.startswith('x'):return chr(int(text[1:],16))
  return {'b':'\b','f':'\f','n':'\n','r':'\r','t':'\t','v':'\v'}.get(text,text)
 return re.sub(r'\\([0-7]{1,3}|x[0-9a-fA-F]{1,2}|.)',one,value)

def encrypted_rows(text):
 active=False;rows=[];columns=None
 for line in text.splitlines():
  if line.startswith('COPY '):
   m=re.fullmatch(r'COPY public\.api_cloud_github_users \(([^)]+)\) FROM stdin;',line)
   if not m:raise ValueError('zuse_rehearsal_unexpected_dump_table')
   columns=[s.strip().strip('"') for s in m.group(1).split(',')]
   if 'account_id' not in columns or 'sealed_credentials' not in columns:raise ValueError('zuse_rehearsal_legacy_crypto_columns_missing')
   active=True;continue
  if not active:continue
  if line==r'\.':active=False;continue
  fields=line.split('\t')
  if len(fields)!=len(columns):raise ValueError('zuse_rehearsal_legacy_crypto_copy_invalid')
  rows.append({'accountId':unescape(fields[columns.index('account_id')]),'sealed':unescape(fields[columns.index('sealed_credentials')])})
 if active:raise ValueError('zuse_rehearsal_legacy_crypto_copy_truncated')
 return rows[:64]

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--export-directory',type=Path,required=True);p.add_argument('--approve-source-fingerprint',required=True);p.add_argument('--primary-key-env',type=Path,required=True);p.add_argument('--fallback-key-env',type=Path);a=p.parse_args()
 manifest,dump=retained.validate_export(a.export_directory,a.approve_source_fingerprint)
 report_path=a.export_directory/'encryption-proof-report.json'
 if report_path.exists():raise ValueError('zuse_rehearsal_crypto_report_already_exists')
 text=retained.run(['pg_restore','--data-only','--schema=public','--table=api_cloud_github_users','--file=-',str(dump)])
 if len(text)>4*1024*1024:raise ValueError('zuse_rehearsal_crypto_sample_too_large')
 rows=encrypted_rows(text)
 primary=retained.encryption_rows_proof(rows,a.primary_key_env)
 fallback=None;match='primary' if primary['keyMatchesAllSamples'] else 'unverified'
 if not primary['keyMatchesAllSamples'] and a.fallback_key_env:
  fallback=retained.encryption_rows_proof(rows,a.fallback_key_env)
  if fallback['keyMatchesAllSamples']:match='fallback'
 report=dict(mode='private-dump-crypto-proof',exportSha256=manifest['sha256'],metadataOnly=True,sampleRows=len(rows),primary=primary,fallback=fallback,matchingConfiguredSource=match,keyMatchVerified=match!='unverified',scope='Legacy GitHub account-bound API envelopes only',sourceConnectionOpened=False,databaseOrContainerStarted=False,sourceWrites=False,customerRecordsPrinted=False)
 retained.private_write(report_path,json.dumps(report,sort_keys=True,indent=2));print(json.dumps(report,sort_keys=True))

if __name__=='__main__':
 try:main()
 except Exception as error:
  code=str(error) if re.fullmatch(r'zuse_rehearsal_[a-z0-9_]+',str(error)) else 'zuse_rehearsal_private_dump_crypto_failed'
  print(json.dumps({'ok':False,'code':code,'customerRecordsPrinted':False}));raise SystemExit(1)
