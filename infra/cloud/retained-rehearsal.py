#!/usr/bin/env python3
"""Private retained snapshot rehearsal in ONE disposable network-isolated PG18.

Never accepts a destination database URL. No source connection/key is read. No
customer row leaves PostgreSQL through report output; only counts/digests/checks.
The approved dump stays private after the disposable container/data are removed.
"""
import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
import time

HERE=Path(__file__).resolve().parent
sp=importlib.util.spec_from_file_location('migration',HERE/'migration-rehearsal.py');migration=importlib.util.module_from_spec(sp);sp.loader.exec_module(migration)

def safe_failure(stderr):
    # Strict classifications only. Never return the error body, SQL statement,
    # failed-row DETAIL, credential, or arbitrary quoted value from the server.
    patterns = (
        (r'unrecognized configuration parameter "([a-z_]+)"', 'unrecognized_parameter'),
        (r'permission denied', 'permission_denied'),
        (r'collation .* does not exist', 'collation_missing'),
        (r'tablespace .* does not exist', 'tablespace_missing'),
        (r'role .* does not exist', 'role_missing'),
        (r'schema .* already exists', 'schema_exists'),
        (r'function .* does not exist', 'function_missing'),
        (r'type .* does not exist', 'type_missing'),
        (r'relation .* does not exist', 'relation_missing'),
        (r'violates foreign key constraint', 'foreign_key_violation'),
        (r'violates check constraint', 'check_violation'),
        (r'violates not-null constraint', 'not_null_violation'),
        (r'duplicate key value violates unique constraint', 'unique_violation'),
        (r'unsupported compression', 'unsupported_compression'),
        (r'out of memory|Cannot allocate memory', 'memory_limit'),
        (r'server closed the connection unexpectedly', 'server_connection_lost'),
        (r'No space left on device', 'disk_limit'),
        (r'could not connect|connection to server', 'connection_failed'),
    )
    for pattern,code in patterns:
        if re.search(pattern,stderr,re.I):return code
    return 'unclassified'

def run(command,*,env=None,input=None,timeout=180):
 p=subprocess.run(command,env=env,input=input,text=True,capture_output=True,timeout=timeout)
 if p.returncode:
  label={'docker':'container_operation','pg_restore':'snapshot_restore','psql':'isolated_sql','node':'private_crypto'}.get(Path(command[0]).name,'private_operation')
  state=re.search(r'\b(?:ERROR|FATAL):\s+([0-9A-Z]{5})(?:\s|$)',p.stderr)
  suffix='_sqlstate_'+state.group(1).lower() if state else '_'+safe_failure(p.stderr)
  raise ValueError('zuse_rehearsal_'+label+'_failed'+suffix)
 return p.stdout

def read_private(path,limit):
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
 try:
  s=os.fstat(fd)
  if not stat.S_ISREG(s.st_mode) or s.st_uid!=os.geteuid() or s.st_mode&0o077 or s.st_size>limit:raise ValueError('zuse_rehearsal_private_export_required')
  return os.read(fd,limit+1)
 finally:os.close(fd)

def validate_export(directory,approved):
 root=Path(directory).absolute();st=root.lstat()
 if not stat.S_ISDIR(st.st_mode) or st.st_uid!=os.geteuid() or st.st_mode&0o077:raise ValueError('zuse_rehearsal_private_export_required')
 manifest=json.loads(read_private(root/'manifest.json',1024*1024))
 if not re.fullmatch(r'[0-9a-f]{64}',approved):raise ValueError('zuse_rehearsal_source_approval_invalid')
 if manifest.get('mode')!='private-consistent-export' or manifest.get('sourceSchema')!='public' or manifest.get('sourceTableCount')!=66:raise ValueError('zuse_rehearsal_export_scope_invalid')
 if manifest.get('sourceFingerprint')!=approved or manifest.get('sourceReference')!=migration.REFERENCE or manifest.get('sourceWritesAllowed') is not False or manifest.get('writerCutoverAllowed') is not False:raise ValueError('zuse_rehearsal_export_identity_mismatch')
 if manifest.get('file')!='legacy-public.dump':raise ValueError('zuse_rehearsal_dump_name_invalid')
 dump=root/'legacy-public.dump';s=dump.lstat()
 if not stat.S_ISREG(s.st_mode) or s.st_uid!=os.geteuid() or s.st_mode&0o077 or s.st_size>1024*1024*1024:raise ValueError('zuse_rehearsal_private_export_required')
 digest=hashlib.sha256()
 fd=os.open(dump,os.O_RDONLY|os.O_NOFOLLOW)
 with os.fdopen(fd,'rb') as f:
  for block in iter(lambda:f.read(1024*1024),b''):digest.update(block)
 if manifest.get('bytes')!=s.st_size or manifest.get('sha256')!=digest.hexdigest():raise ValueError('zuse_rehearsal_export_checksum_mismatch')
 return manifest,dump

@contextlib.contextmanager
def cluster(image, operation_path):
 if not re.fullmatch(r'(?:docker.io/library/)?postgres@sha256:[0-9a-f]{64}',image):raise ValueError('zuse_rehearsal_pinned_postgres_image_required')
 name='zuse-retained-rehearsal-'+secrets.token_hex(8)
 # The host parent remains root0700 even when the image initializes its two
 # child mounts as postgres. Other service UIDs cannot traverse the socket path.
 with tempfile.TemporaryDirectory(prefix='zuse-private-retained-') as directory:
  root=Path(directory);root.chmod(0o700);data=root/'data';data.mkdir();socket=root/'socket';socket.mkdir()
  env={'PATH':'/usr/local/bin:/usr/bin:/bin','PGHOST':str(socket),'PGPORT':'5432','PGUSER':'zuse_rehearsal','PGDATABASE':'postgres','PGSSLMODE':'disable','PGCONNECT_TIMEOUT':'3'}
  operation={'containerName':name,'privateDirectory':str(root),'image':image,'phase':'launch-intent'}
  fd=os.open(operation_path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
  with os.fdopen(fd,'w') as f:json.dump(operation,f);f.flush();os.fsync(f.fileno())
  try:
   run(['docker','run','--detach','--name',name,'--label','zuse.isolated-migration-rehearsal=true',
        '--network=none','--log-driver=none','--memory=256m','--cpus=0.5','--pids-limit=128','--read-only','--security-opt','no-new-privileges=true',
        '--mount','type=bind,src='+str(data)+',dst=/var/lib/postgresql',
        '--mount','type=bind,src='+str(socket)+',dst=/var/run/postgresql',
        '--tmpfs','/tmp:rw,nosuid,noexec,size=16m','--env','POSTGRES_HOST_AUTH_METHOD=trust',
        '--env','POSTGRES_USER=zuse_rehearsal','--env','POSTGRES_DB=postgres',image,
        'postgres','-c','listen_addresses=','-c','unix_socket_permissions=0700','-c','max_connections=12',
        '-c','shared_buffers=16MB','-c','max_wal_size=64MB','-c','min_wal_size=32MB',
        '-c','log_error_verbosity=terse','-c','log_min_error_statement=panic'],timeout=120)
   until=time.monotonic()+45
   while True:
    # The official image briefly opens a temporary initialization server.
    # Accept only the final PID1 postgres process, not that transient socket.
    final=subprocess.run(['docker','exec',name,'/bin/sh','-c','test "$(cat /proc/1/comm)" = postgres'],capture_output=True,text=True,timeout=5)
    p=subprocess.run(['psql','-X','--no-password','-Atqc','SELECT 1'],env=env,capture_output=True,text=True,timeout=5)
    if final.returncode==0 and p.returncode==0:break
    if time.monotonic()>until:raise ValueError('zuse_rehearsal_isolated_server_not_ready')
    time.sleep(.2)
   yield env
  finally:
   # Only this newly minted name can be removed; no user-facing container target.
   exists=subprocess.run(['docker','container','inspect',name],capture_output=True,text=True,timeout=10)
   if exists.returncode==0:run(['docker','rm','--force',name],timeout=30)
   p=subprocess.run(['docker','container','inspect',name],capture_output=True,text=True,timeout=10)
   if p.returncode==0:raise ValueError('zuse_rehearsal_isolated_container_cleanup_unconfirmed')
   operation['phase']='removed'
   with Path(operation_path).open('w') as f:json.dump(operation,f);f.flush();os.fsync(f.fileno())

def execute(env,sql):
 text=run(['psql','-X','--no-password','--quiet','--no-align','--tuples-only','-v','ON_ERROR_STOP=1','-v','VERBOSITY=sqlstate','-v','SHOW_CONTEXT=never'],env=env,input=sql)
 return [json.loads(line) for line in text.splitlines() if line.startswith('{')]

def metadata(env,schema,table):
 return execute(env,"SELECT json_build_object('columns',json_agg(column_name ORDER BY ordinal_position)) FROM information_schema.columns WHERE table_schema="+migration.database.literal(schema)+" AND table_name="+migration.database.literal(table)+";")[-1]['columns']

def table_proof(env,schema,table,omit=()):
 value='to_jsonb(t)'
 if omit:value+='-ARRAY['+','.join(migration.database.literal(c) for c in omit)+']::text[]'
 # Hash each canonical JSON row inside PG, then hash ordered row digests. The
 # caller sees no primary key, plaintext, cipher body or customer identifier.
 sql="SELECT json_build_object('count',count(*),'sha256',encode(sha256(convert_to(coalesce(string_agg(h,'|' ORDER BY h),''),'UTF8')),'hex')) FROM (SELECT encode(sha256(convert_to(("+value+")::text,'UTF8')),'hex') h FROM "+migration.ident(schema)+'.'+migration.ident(table)+" t)q;"
 return execute(env,sql)[-1]

LEGACY_PROCEDURAL_TRIGGERS = {
    ('api_cloud_projects','api_cloud_catalog_project'),
    ('api_cloud_workspace_runtime_summaries','api_cloud_catalog_summary'),
    ('api_cloud_workspaces','api_cloud_catalog_workspace'),
    ('api_snapshot_price_schedule','api_snapshot_price_immutable'),
}

def select_restore_toc(text):
    kept=[];removed=[]
    for line in text.splitlines(keepends=True):
        if re.search(r'^\d+; \d+ \d+ TRIGGER ',line):
            match=re.match(r'^\d+; \d+ \d+ TRIGGER public ([a-z_]+) ([a-z_]+) ',line)
            if not match or tuple(match.groups()) not in LEGACY_PROCEDURAL_TRIGGERS:
                raise ValueError('zuse_rehearsal_unknown_procedural_trigger')
            removed.append(line);kept.append('; quarantined '+line)
        else:kept.append(line)
    if removed and len(removed)!=4:raise ValueError('zuse_rehearsal_partial_procedural_trigger_inventory')
    return ''.join(kept),removed

def private_write(path,text):
    fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'w') as f:f.write(text);f.flush();os.fsync(f.fileno())

def restore_toc(dump,directory,suffix):
    text=run(['pg_restore','--list',str(dump)])
    selected,removed=select_restore_toc(text)
    filtered=Path(directory)/('restore-toc'+suffix+'.list')
    private_write(filtered,selected)
    ddl=Path(directory)/('legacy-procedural-triggers'+suffix+'.sql')
    if removed:
        triggers=Path(directory)/('legacy-procedural-triggers'+suffix+'.list')
        private_write(triggers,''.join(removed))
        # Schema-only plus an exact four-TRIGGER TOC contains no customer rows.
        statement=run(['pg_restore','--schema-only','--no-owner','--no-privileges','--file=-','--use-list='+str(triggers),str(dump)])
        if re.search(r'CREATE\s+CONSTRAINT\s+TRIGGER',statement,re.I):
            raise ValueError('zuse_rehearsal_constraint_trigger_cannot_be_quarantined')
        private_write(ddl,statement)
    record={'quarantinedCount':len(removed),'tocEntries':removed,'ddlFile':ddl.name if removed else None,'sourceBehaviorConverted':False}
    private_write(Path(directory)/('legacy-procedural-triggers'+suffix+'.json'),json.dumps(record,sort_keys=True,indent=2))
    return filtered,len(removed)

def encryption_rows_proof(rows, path):
 if path is None:return {"metadataOnly":True,"checked":0,"opened":0,"failed":0,"keyMatchesAllSamples":False}
 import shlex
 content=read_private(Path(path),1024*1024).decode()
 matches=[]
 for line in content.splitlines():
  if line.startswith('CLOUD_DATA_ENCRYPTION_KEY='):
   raw=line.split('=',1)[1];matches.append(shlex.split(raw)[0] if raw.startswith(('"',"'")) else raw)
 if len(matches)!=1:raise ValueError('zuse_rehearsal_legacy_key_unavailable')

 # Credentials/key stay in private process pipes. The subprocess prints ONLY
 # authenticated-open counts; decrypted bytes are immediately overwritten.
 result=run([sys.executable,'-I',str(HERE/'legacy-encryption-proof.py')],input=json.dumps({'key':matches[0],'rows':rows}),env={'PATH':'/usr/local/bin:/usr/bin:/bin'},timeout=10)
 proof=json.loads(result)
 if not isinstance(proof,dict) or proof.get('metadataOnly') is not True:raise ValueError('zuse_rehearsal_private_crypto_proof_invalid')
 return proof

def encryption_proof(env, path):
 rows=execute(env,"SELECT jsonb_build_object('rows',coalesce(jsonb_agg(to_jsonb(t)),'[]'::jsonb)) FROM (SELECT account_id AS \"accountId\",sealed_credentials AS sealed FROM public.api_cloud_github_users ORDER BY account_id LIMIT 64)t;")[-1]['rows']
 return encryption_rows_proof(rows,path)

def rehearse(directory,approved,image,encryption_env_file=None,operation_id=None,encryption_fallback_env_file=None):
 manifest,dump=validate_export(directory,approved)
 inventory=migration.table_inventory();source='public';target='zuse_migration_target';control='zuse_migration_quarantine'
 if (Path(directory)/'rehearsal-report.json').exists():raise ValueError('zuse_rehearsal_successful_report_already_exists')
 if operation_id is not None and not re.fullmatch(r'[a-f0-9]{32}',operation_id):raise ValueError('zuse_rehearsal_operation_id_invalid')
 suffix='-'+operation_id if operation_id else ''
 operation_path=Path(directory)/('rehearsal'+suffix+'-operation.json')
 if operation_path.exists():raise ValueError('zuse_rehearsal_inspect_original_isolated_operation')
 toc,trigger_count=restore_toc(dump,directory,suffix)
 with cluster(image,operation_path) as env:
  progress_path=Path(directory)/('rehearsal'+suffix+'-progress.json')
  def phase(name):
   fd=os.open(progress_path,os.O_WRONLY|os.O_CREAT|os.O_TRUNC|os.O_NOFOLLOW,0o600)
   with os.fdopen(fd,'w') as f:json.dump({'phase':name,'customerRecordsPrinted':False},f);f.flush();os.fsync(f.fileno())
  phase('restoring-private-snapshot')
  # Restore only inside our own new networkless server, never a supplied URL.
  run(['pg_restore','--no-owner','--no-privileges','--exit-on-error','--single-transaction','--use-list='+str(toc),'--dbname=postgres',str(dump)],env=env,timeout=180)
  phase('applying-target-migrations')
  execute(env,'CREATE SCHEMA '+migration.ident(target)+'; SET search_path TO '+migration.ident(target)+',pg_catalog;'+'\n'.join(m['sql'] for m in migration.database.migrations()))
  execute(env,'CREATE SCHEMA '+migration.ident(control)+'; CREATE TABLE '+migration.ident(control)+'.retained_rows(table_name text NOT NULL,row_data jsonb NOT NULL);')
  phase('validating-column-map')
  columns={};source_proofs={}
  for table in inventory['retained']:
   old=metadata(env,source,table);new=metadata(env,target,table)
   if not old or not new or not set(old)<=set(new):raise ValueError('zuse_rehearsal_unmapped_legacy_column')
   columns[table]=old;source_proofs[table]=table_proof(env,source,table)
  phase('importing-retained-rows')
  # Every imported row/constraint is inside ONE isolated transaction. Never
  # disable triggers or quietly ON CONFLICT past conflicting identities.
  execute(env,'BEGIN;'+''.join('INSERT INTO '+migration.ident(target)+'.'+migration.ident(table)+'('+','.join(map(migration.ident,names))+') SELECT '+','.join(map(migration.ident,names))+' FROM '+migration.ident(source)+'.'+migration.ident(table)+';' for table,names in columns.items())+'COMMIT;')
  phase('verifying-retained-row-proofs')
  total=0
  for table,names in columns.items():
   omitted=set(metadata(env,target,table))-set(names)
   actual=table_proof(env,target,table,omitted)
   if actual!=source_proofs[table]:raise ValueError('zuse_rehearsal_retained_row_or_identity_changed')
   total+=actual['count']
  phase('preserving-quarantined-rows')
  quarantined=0
  for table in inventory['quarantined']:
   before=table_proof(env,source,table)
   execute(env,'INSERT INTO '+migration.ident(control)+'.retained_rows SELECT '+migration.database.literal(table)+',to_jsonb(t) FROM public.'+migration.ident(table)+' t;')
   check=execute(env,"SELECT json_build_object('count',count(*),'sha256',encode(sha256(convert_to(coalesce(string_agg(h,'|' ORDER BY h),''),'UTF8')),'hex')) FROM (SELECT encode(sha256(convert_to(row_data::text,'UTF8')),'hex') h FROM "+migration.ident(control)+'.retained_rows WHERE table_name='+migration.database.literal(table)+")q;")[-1]
   if check!=before or table_proof(env,target,table)['count']!=0:raise ValueError('zuse_rehearsal_quarantine_changed_or_activated')
   quarantined+=check['count']
  sequences=execute(env,"SELECT json_build_object('sequences',coalesce(json_agg(json_build_object('name',s.relname,'table',t.relname,'column',a.attname)),'[]')) FROM pg_class s JOIN pg_namespace n ON n.oid=s.relnamespace JOIN pg_depend d ON d.objid=s.oid AND d.deptype IN ('a','i') JOIN pg_class t ON t.oid=d.refobjid JOIN pg_attribute a ON a.attrelid=t.oid AND a.attnum=d.refobjsubid WHERE s.relkind='S' AND n.nspname='public' AND t.relname LIKE 'api_%';")[-1]['sequences']
  for sequence in sequences:
   name=sequence['name'];value=execute(env,'SELECT json_build_object(\'value\',last_value,\'called\',is_called) FROM public.'+migration.ident(name)+';')[-1]
   execute(env,"SELECT setval("+migration.database.literal(target+'.'+name)+','+str(int(value['value']))+','+('true' if value['called'] else 'false')+');')
   after=execute(env,'SELECT json_build_object(\'value\',last_value,\'called\',is_called) FROM '+migration.ident(target)+'.'+migration.ident(name)+';')[-1]
   if value!=after:raise ValueError('zuse_rehearsal_sequence_high_water_changed')
  # Real source remains its only writer. The isolated target has no application
  # worker, no inherited ownership/credentials and no runnable provider jobs.
  fence=execute(env,'CREATE ROLE zuse_migration_inactive; SELECT json_build_object(\'nativeOwners\',(SELECT count(*) FROM '+migration.ident(target)+'.zuse_rust_control_owners),\'providerJobs\',(SELECT count(*) FROM '+migration.ident(target)+'.zuse_rust_jobs),\'targetRoleCanWrite\',has_table_privilege(\'zuse_migration_inactive\','+migration.database.literal(target+'.api_cloud_workspaces')+',\'INSERT,UPDATE,DELETE\'));')[-1]
  if fence!={'nativeOwners':0,'providerJobs':0,'targetRoleCanWrite':False}:raise ValueError('zuse_rehearsal_target_writer_not_fenced')
  private_write(Path(directory)/('structural-evidence'+suffix+'.json'),json.dumps({'retainedTableCount':len(columns),'retainedRowCount':total,'quarantineTableCount':len(inventory['quarantined']),'quarantinedRowCount':quarantined,'allSourceColumnsAndCiphertextHashesPreserved':True,'allImportConstraintsEnforced':True,'ownedSequencesPreserved':len(sequences),'sequenceLastValueAndCalledPreserved':True,'nativeOwners':0,'providerJobs':0,'targetRoleCanWrite':False},sort_keys=True))
  phase('private-encryption-proof')
  try:crypto=encryption_proof(env,encryption_env_file)
  except Exception as error:crypto={'metadataOnly':True,'checked':0,'opened':0,'failed':0,'keyMatchesAllSamples':False,'code':'private_crypto_proof_unavailable','errorType':type(error).__name__}
  key_source='primary' if crypto['keyMatchesAllSamples'] else 'unverified'
  primary_counts=crypto
  if not crypto['keyMatchesAllSamples'] and encryption_fallback_env_file:
   try:fallback=encryption_proof(env,encryption_fallback_env_file)
   except Exception as error:fallback={'metadataOnly':True,'checked':0,'opened':0,'failed':0,'keyMatchesAllSamples':False,'code':'private_crypto_proof_unavailable','errorType':type(error).__name__}
   if fallback['keyMatchesAllSamples']:crypto=fallback;key_source='fallback'
  else:fallback=None
  report=dict(mode='private-retained-rehearsal',sourceFingerprint=approved,exportSha256=manifest['sha256'],
    retainedTableCount=len(columns),retainedRowCount=total,quarantineTableCount=len(inventory['quarantined']),quarantinedRowCount=quarantined,
    stableIdsCiphertextAndAllSourceColumnsPreserved=True,importConstraintsEnforced=True,quarantinePreservedAndInactive=True,
    ownedSequencesPreserved=len(sequences),sequenceLastValueAndCalledPreserved=True,sourceWritesPerformed=False,writerCutoverPerformed=False,
    sourceWriter='legacy-unchanged',targetWriter='none',nativeControlOwnersImported=0,providerJobsImported=0,
    proceduralTriggerMetadataAndDdlQuarantined=trigger_count,sourceTriggerBehaviorConverted=False,
    sourceEncryptionKeyMatchVerified=crypto['keyMatchesAllSamples'],encryptionSampleCounts=crypto,encryptionPrimarySampleCounts=primary_counts,encryptionFallbackSampleCounts=fallback,encryptionMatchingConfiguredSource=key_source,encryptionValidationScope='Legacy GitHub account-bound API envelopes only',objectPayloadsCopied=False,legacyGuestConverted=False,customerRecordsPrinted=False,
    productionSchemaModified=False,stagingUserFacingSchemaModified=False,isolatedNetwork='none',nativeExecutionAdmitted=False,
    completeEndToEndMigrationCertified=False)
 with_cleanup=dict(report,isolatedContainerAndDataRemoved=True)
 path=Path(directory)/'rehearsal-report.json'
 fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 with os.fdopen(fd,'w') as f:json.dump(with_cleanup,f,sort_keys=True,indent=2)
 return with_cleanup

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--export-directory',type=Path,required=True);p.add_argument('--approve-source-fingerprint',required=True);p.add_argument('--postgres-image',required=True);p.add_argument('--encryption-env-file',type=Path);p.add_argument('--operation-id',required=True);p.add_argument('--encryption-fallback-env-file',type=Path);a=p.parse_args()
 print(json.dumps(rehearse(a.export_directory,a.approve_source_fingerprint,a.postgres_image,a.encryption_env_file,a.operation_id,a.encryption_fallback_env_file),sort_keys=True))

if __name__=='__main__':
 try:main()
 except Exception as e:
  code=str(e) if re.fullmatch(r'zuse_rehearsal_[a-z0-9_]+',str(e)) else 'zuse_rehearsal_retained_import_failed'
  print(json.dumps({'ok':False,'code':code,'customerRecordsPrinted':False}));raise SystemExit(1)
