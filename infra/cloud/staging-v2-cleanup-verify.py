#!/usr/bin/env python3
"""Independent READ ONLY staging inventory/cleanup check; no provider mutations.

Private DSN file contains only a PostgreSQL URL. Snapshot targets BEFORE Delete;
metadata purge can remove allocation rows. After Delete recheck SQL and ask the
provider directly for every original machine ID. Owner/JWT is never needed here.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess

HERE=Path(__file__).resolve().parent

def module(name):
 s=importlib.util.spec_from_file_location(name,HERE/(name+'.py'));m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m

def private(path):
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
 try:
  st=os.fstat(fd)
  if not stat.S_ISREG(st.st_mode) or st.st_uid!=os.geteuid() or st.st_mode&0o077 or st.st_size>65536:raise ValueError('private_file_required')
  return os.read(fd,65537).decode().strip()
 finally:os.close(fd)

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('action',choices=('inventory','verify'))
 p.add_argument('--workspace',required=True);p.add_argument('--owner',required=True)
 p.add_argument('--database-url-file',type=Path,required=True);p.add_argument('--targets-file',type=Path,required=True)
 p.add_argument('--provider-env-file',type=Path);p.add_argument('--boxd-sdk');p.add_argument('--node-binary',default='node');a=p.parse_args()
 if not re.fullmatch(r'workspace_[a-f0-9]{32}',a.workspace) or not re.fullmatch(r'user_[A-Za-z0-9]+',a.owner):raise ValueError('exact_owner_workspace_required')
 db=module('database');device=module('staging-owner-device')
 url=private(a.database_url_file)
 # Literal values are strictly validated; no caller-controlled table/schema/SQL.
 query="""BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
 SET LOCAL search_path=zuse_staging,pg_catalog; SET LOCAL statement_timeout='10s';
 SELECT jsonb_build_object('workspaceId',w.workspace_id,'owner',w.account_id,'provider',w.provider,
 'state',w.state,'desiredState',w.desired_state,'providerReferenceCleared',w.provider_sandbox_id IS NULL,
 'credentialHashesCleared',w.runtime_credential_hash IS NULL AND w.runtime_boot_token_hash IS NULL,
 'transcriptKeyCleared',w.wrapped_transcript_key IS NULL,
 'deletionPhase',(SELECT phase FROM zuse_rust_deletions WHERE workspace_id=w.workspace_id),
 'pendingJobs',(SELECT count(*) FROM zuse_rust_jobs WHERE resource_id=w.workspace_id AND state IN ('pending','running')),
 'targets',COALESCE((SELECT jsonb_agg(DISTINCT machine_id) FROM (
 SELECT machine_id FROM zuse_rust_allocations WHERE workspace_id=w.workspace_id
 UNION SELECT machine_id FROM zuse_rust_provider_creates WHERE workspace_id=w.workspace_id
 UNION SELECT w.provider_sandbox_id)x WHERE machine_id IS NOT NULL),'[]'::jsonb))
 FROM api_cloud_workspaces w WHERE workspace_id='WORKSPACE' AND account_id='OWNER'; COMMIT;""".replace('WORKSPACE',a.workspace).replace('OWNER',a.owner)
 values=db.execute(url,query)
 if len(values)!=1:raise ValueError('exact_workspace_row_required')
 report=values[0]
 if a.action=='inventory':
  if not report['targets']:raise ValueError('original_provider_identity_not_recorded')
  if a.targets_file.exists():raise ValueError('target_inventory_is_immutable')
  device.write_private(a.targets_file,report)
  print(json.dumps({'metadataOnly':True,'inventorySaved':True,'workspaceId':a.workspace,'targetCount':len(report['targets'])}));return
 inventory=json.loads(private(a.targets_file))
 if inventory['workspaceId']!=a.workspace or inventory['owner']!=a.owner or inventory['provider']!=report['provider']:raise ValueError('target_identity_changed')
 if not (report['state']==report['desiredState']=='deleted' and report['deletionPhase']=='deleted' and report['providerReferenceCleared'] and report['credentialHashesCleared'] and report['transcriptKeyCleared'] and report['pendingJobs']==0):raise ValueError('independent_database_cleanup_incomplete')
 if not a.provider_env_file:raise ValueError('private_provider_environment_required')
 import shlex
 env={'PATH':'/usr/local/bin:/usr/bin:/bin'}
 allowed={'boxd':('BOXD_API_KEY','BOXD_ORG'),'boat':('BOAT_API_KEY',),
          'e2b':('E2B_API_KEY',),'daytona':('DAYTONA_API_KEY',)}.get(inventory['provider'])
 if not allowed:raise ValueError('unsupported_provider_inventory')
 for line in private(a.provider_env_file).splitlines():
  if '=' not in line or line.lstrip().startswith('#'):continue
  key,value=line.split('=',1)
  if key in allowed:
   env[key]=shlex.split(value)[0] if value.startswith(('"',"'")) else value
 payload={'provider':inventory['provider'],'targets':inventory['targets'],'sdk':a.boxd_sdk}
 node=subprocess.run([a.node_binary,str(HERE/'staging-v2-provider-absence.mjs')],input=json.dumps(payload),text=True,env=env,capture_output=True,timeout=120)
 if node.returncode:raise ValueError('independent_provider_absence_not_verified')
 evidence=json.loads(node.stdout)
 if evidence.get('allAbsent') is not True:raise ValueError('provider_resource_still_exists')
 print(json.dumps({'metadataOnly':True,'ok':True,'workspaceId':a.workspace,'databaseDeleted':True,'credentialReferencesCleared':True,'pendingJobs':0,'completePreDeleteInventory':inventory.get('completePreDeleteInventory',True),'independentProviderAbsence':evidence}))

if __name__=='__main__':
 try:main()
 except Exception:print(json.dumps({'metadataOnly':True,'ok':False,'code':'independent_cleanup_verification_failed'}));raise SystemExit(1)
