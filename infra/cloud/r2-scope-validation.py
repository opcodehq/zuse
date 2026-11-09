#!/usr/bin/env python3
"""Candidate-only staging S3 scope proof; production access is HEAD-only.

Does not activate credentials, change services or rotate/revoke any token.
Private journals retain original synthetic keys; stdout is counts/status only.
"""
import argparse,hashlib,io,json,logging,os,re,secrets,shlex,stat
from pathlib import Path
STAGING='zuse-rust-staging'
PRODUCTION='zuse-cloud-transcripts'
PREFIX='CLOUD_TRANSCRIPT_R2_'
FIELDS=('ENDPOINT','BUCKET','ACCESS_KEY_ID','SECRET_ACCESS_KEY')
ACTIVE=Path('/etc/zuse/cloud/staging/storage.env')
INVENTORY=Path('/etc/zuse/cloud/staging/migration-rehearsal/legacy-snapshot-a13a32e1c474448f/object-copy-production-read-operation.json')
CANDIDATES=Path('/etc/zuse/cloud/staging/r2-scope-candidates')
class Rejected(Exception):pass
def require(value,code):
 if not value:raise Rejected(code)
def private_read(path,limit=1048576):
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
 with os.fdopen(fd,'rb') as source:
  meta=os.fstat(source.fileno());require(stat.S_ISREG(meta.st_mode) and meta.st_uid==os.getuid() and meta.st_mode&0o077==0 and meta.st_nlink==1 and meta.st_size<=limit,'private_file_unsafe')
  data=source.read(limit+1);require(len(data)<=limit,'private_file_limit');return data
def private_write(path,value,initial=False):
 path=Path(path);meta=path.parent.lstat();require(stat.S_ISDIR(meta.st_mode) and not path.parent.is_symlink() and meta.st_uid==os.getuid() and meta.st_mode&0o077==0,'private_directory_unsafe')
 data=(json.dumps(value,sort_keys=True,indent=2)+'\n').encode()
 if initial:
  fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
  with os.fdopen(fd,'wb') as out:out.write(data);out.flush();os.fsync(out.fileno())
 else:
  private_read(path);temp=path.with_name(path.name+'.next-'+secrets.token_hex(8))
  fd=os.open(temp,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
  try:
   with os.fdopen(fd,'wb') as out:out.write(data);out.flush();os.fsync(out.fileno())
   os.replace(temp,path)
  finally:temp.unlink(missing_ok=True)
def environment(path,wanted=FIELDS):
 result={}
 for line in private_read(path).decode().splitlines():
  if not line.strip() or line.lstrip().startswith('#') or '=' not in line:continue
  key,value=line.split('=',1)
  if key.strip() not in {PREFIX+name for name in wanted}:continue
  require(key.strip() not in result,'duplicate_r2_setting')
  parsed=shlex.split(value,comments=False);require(len(parsed)==1,'r2_setting_invalid');result[key.strip()]=parsed[0]
 require(all(result.get(PREFIX+name) for name in wanted),'r2_configuration_missing')
 endpoint=result[PREFIX+'ENDPOINT'].rstrip('/');require(re.fullmatch(r'https://[a-zA-Z0-9.-]+\.r2\.cloudflarestorage\.com',endpoint) is not None,'r2_endpoint_invalid');result[PREFIX+'ENDPOINT']=endpoint
 require(result[PREFIX+'BUCKET']==STAGING,'staging_bucket_not_pinned')
 return result
def prepare(source,candidate):
 values=environment(source,('ENDPOINT','BUCKET'));require(not Path(candidate).exists(),'candidate_already_exists')
 parent=Path(candidate).parent;meta=parent.lstat();require(stat.S_ISDIR(meta.st_mode) and meta.st_uid==os.getuid() and meta.st_mode&0o077==0 and not parent.is_symlink(),'private_directory_unsafe')
 fd=os.open(candidate,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 with os.fdopen(fd,'w') as out:
  for name in ('ENDPOINT','BUCKET'):out.write(PREFIX+name+'='+shlex.quote(values[PREFIX+name])+'\n')
 return {'status':'candidate-prepared','scope':STAGING,'activeCredentialChanged':False}
def sdk(values):
 import boto3
 from botocore.config import Config
 return boto3.client('s3',endpoint_url=values[PREFIX+'ENDPOINT'],region_name='auto',aws_access_key_id=values[PREFIX+'ACCESS_KEY_ID'],aws_secret_access_key=values[PREFIX+'SECRET_ACCESS_KEY'],verify=True,config=Config(signature_version='s3v4',connect_timeout=5,read_timeout=10,retries={'total_max_attempts':1},s3={'addressing_style':'path'}))
def status(error):
 response=getattr(error,'response',{}) or {};code=str(response.get('Error',{}).get('Code',''));number=response.get('ResponseMetadata',{}).get('HTTPStatusCode')
 if number is None:
  if code in ('404','NoSuchKey','NotFound'):number=404
  elif code in ('403','AccessDenied','Forbidden'):number=403
 return number,code
def head(client,bucket,key):
 try:return 200,client.head_object(Bucket=bucket,Key=key),''
 except Exception as error:
  number,code=status(error)
  if number in (403,404):return number,None,code
  raise Rejected('s3_head_unavailable') from None
def control_object(path):
 value=json.loads(private_read(path));require(value.get('sourceBucket')==PRODUCTION,'production_control_bucket_invalid')
 entry=value.get('entries',[None])[0];require(isinstance(entry,dict),'production_control_missing')
 key=entry.get('sourceKey');require(isinstance(key,str) and 0<len(key)<=4096 and '\0' not in key,'production_control_invalid')
 require(isinstance(entry.get('sourceETag'),str) and isinstance(entry.get('bytes'),int) and entry['bytes']>0,'production_control_metadata_missing')
 return {'key':key,'etag':entry['sourceETag'],'bytes':entry['bytes']}
def own_key(key):return isinstance(key,str) and re.fullmatch(r'scope-verification/[0-9a-f]{32}/roundtrip\.bin',key) is not None
def bounded_body(value,limit):
 body=value['Body']
 try:
  data=body.read(limit+1);require(len(data)<=limit and not body.read(1),'synthetic_read_limit');return data
 finally:body.close()
def cleanup(candidate,journal,save):
 key=journal['syntheticKey'];require(journal['stagingBucket']==STAGING and own_key(key),'cleanup_scope_invalid')
 code,metadata,_=head(candidate,STAGING,key)
 if code==404:
  journal['cleanupHeadStatus']=404
  # Absence cannot settle an earlier PUT whose reply was lost/unobserved.
  journal['cleanupVerified']=not journal.get('putUnobserved',False);save();return
 require(code==200,'cleanup_inspection_unavailable')
 value=candidate.get_object(Bucket=STAGING,Key=key);body=bounded_body(value,journal['bytes'])
 require(len(body)==journal['bytes'] and hashlib.sha256(body).hexdigest()==journal['syntheticSha256'],'cleanup_object_not_owned')
 journal['phase']='deleting';save() # Original delete intent before request.
 try:candidate.delete_object(Bucket=STAGING,Key=key);journal['deleteReply']='completed'
 except Exception:journal['deleteReply']='uncertain'
 code,_,_=head(candidate,STAGING,key);journal['cleanupHeadStatus']=code;journal['cleanupVerified']=code==404;save()
def public_report(journal):
 return {'version':1,'status':'passed' if journal.get('roundtripVerified') and journal.get('cleanupVerified') and journal.get('productionHeadStatus')==403 else 'rejected','scope':STAGING,'productionControlBucket':PRODUCTION,'productionHeadStatus':journal.get('productionHeadStatus'),'productionHeads':journal.get('productionHeads',0),'productionGets':0,'productionWrites':0,'syntheticBytes':journal.get('bytes',0),'stagingPutRequests':journal.get('putRequests',0),'cleanupHeadStatus':journal.get('cleanupHeadStatus'),'cleanupVerified':journal.get('cleanupVerified',False),'errorCode':journal.get('errorCode'),'activeCredentialChanged':False,'servicesRestarted':False}
def validate(candidate,control,known,journal_path,fingerprint):
 journal={'version':1,'phase':'intent','stagingBucket':STAGING,'candidateSha256':fingerprint,'syntheticKey':'scope-verification/'+secrets.token_hex(16)+'/roundtrip.bin','bytes':256,'productionHeads':0,'putRequests':0,'cleanupVerified':False}
 private_write(journal_path,journal,initial=True)
 save=lambda:private_write(journal_path,journal)
 eligible=False
 try:
  code,metadata,_=head(control,PRODUCTION,known['key']);journal['productionHeads']+=1
  require(code==200 and metadata.get('ETag')==known['etag'] and metadata.get('ContentLength')==known['bytes'],'production_control_not_confirmed')
  code,_,error=head(candidate,PRODUCTION,known['key']);journal['productionHeads']+=1;journal['productionHeadStatus']=code;save()
  require(code==403 and error in ('403','AccessDenied','Forbidden'),'production_head_not_denied')
  code,_,_=head(candidate,STAGING,journal['syntheticKey']);require(code==404,'staging_probe_key_not_absent')
  header=b'zuse-staging-scope-v1\n';body=header+secrets.token_bytes(256-len(header));require(len(body)==256,'synthetic_size_invalid')
  journal['syntheticSha256']=hashlib.sha256(body).hexdigest();journal['phase']='putting';journal['putRequests']=1;save();eligible=True
  try:candidate.put_object(Bucket=STAGING,Key=journal['syntheticKey'],Body=body,ContentLength=len(body),ContentType='application/octet-stream',IfNoneMatch='*');journal['putReply']='completed'
  except Exception:journal['putReply']='uncertain';journal['putUnobserved']=True
  save();code,_,_=head(candidate,STAGING,journal['syntheticKey']);require(code==200,'put_outcome_not_observed')
  value=candidate.get_object(Bucket=STAGING,Key=journal['syntheticKey']);actual=bounded_body(value,256)
  require(actual==body,'synthetic_roundtrip_mismatch');journal['roundtripVerified']=True;journal['putUnobserved']=False;journal['phase']='read-verified';save()
 except Rejected as error:journal['errorCode']=str(error)
 except Exception:journal['errorCode']='scope_check_unavailable'
 finally:
  if eligible:
   try:cleanup(candidate,journal,save)
   except Rejected as error:journal['cleanupError']=str(error)
   except Exception:journal['cleanupError']='cleanup_unavailable'
  journal['phase']='finished' if public_report(journal)['status']=='passed' else 'inspection-required';save()
 return public_report(journal)
def main():
 parser=argparse.ArgumentParser(description=__doc__);sub=parser.add_subparsers(dest='mode',required=True)
 prep=sub.add_parser('prepare');prep.add_argument('--source',type=Path,default=ACTIVE);prep.add_argument('--candidate',type=Path,required=True)
 check=sub.add_parser('validate');check.add_argument('--candidate',type=Path,required=True);check.add_argument('--control',type=Path,default=ACTIVE);check.add_argument('--inventory',type=Path,default=INVENTORY);check.add_argument('--journal',type=Path,required=True)
 args=parser.parse_args();logging.disable(logging.CRITICAL);os.umask(0o077);require(os.geteuid()==0,'root_required')
 require(args.candidate.name=='candidate.env' and args.candidate.parent.parent==CANDIDATES and not args.candidate.parent.is_symlink(),'candidate_path_not_pinned')
 if args.mode=='prepare':report=prepare(args.source,args.candidate)
 else:
  require(args.control==ACTIVE and args.inventory==INVENTORY and args.journal.parent==args.candidate.parent and args.journal.name=='scope.intent.json','scope_inputs_not_pinned')
  values=environment(args.candidate);baseline=environment(args.control);require(values[PREFIX+'ENDPOINT']==baseline[PREFIX+'ENDPOINT'],'r2_account_mismatch')
  report=validate(sdk(values),sdk(baseline),control_object(args.inventory),args.journal,hashlib.sha256(private_read(args.candidate)).hexdigest())
 print(json.dumps(report,sort_keys=True));return 0 if report['status'] in ('passed','candidate-prepared') else 1
if __name__=='__main__':
 try:raise SystemExit(main())
 except Exception:print(json.dumps({'status':'rejected','errorCode':'scope_validation_preflight_failed','activeCredentialChanged':False}));raise SystemExit(1)
