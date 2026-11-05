#!/usr/bin/env python3
"""Bounded ciphertext retention proof; never decrypts or writes source objects.

Inventory consumes the previously approved dump, not a database connection.
Private mappings remain beside that dump; stdout contains counts/status only.
"""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import tempfile
import time
from urllib.parse import quote

HERE = Path(__file__).resolve().parent
def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec); spec.loader.exec_module(value)
    return value

retained = module('retained', HERE / 'retained-rehearsal.py')
crypto = module('dump_crypto', HERE / 'legacy-dump-crypto.py')
MAX_BYTES = 25 * 1024 * 1024
TABLES = ('api_cloud_transcript_checkpoints', 'api_cloud_workspaces',
          'api_cloud_workspace_api_messages', 'api_cloud_workspace_launch_intents')
SOURCE_BUCKETS = ('zuse-cloud-transcripts', 'zuse-cloud-transcripts-staging')
HTTP_FIELDS = ('ContentType', 'CacheControl', 'ContentDisposition', 'ContentEncoding',
               'ContentLanguage', 'Expires', 'Metadata')

def copy_rows(text, table):
    active = False; columns = []; rows = []
    for line in text.splitlines():
        if line.startswith('COPY '):
            match = re.fullmatch(r'COPY public\.' + table + r' \(([^)]+)\) FROM stdin;', line)
            if not match: raise ValueError('unexpected_dump_table')
            columns = [s.strip().strip('"') for s in match[1].split(',')]
            active = True
        elif active and line == r'\.': active = False
        elif active:
            values = line.split('\t')
            if len(values) != len(columns): raise ValueError('invalid_copy_row')
            rows.append(dict(zip(columns, map(crypto.unescape, values))))
    if active: raise ValueError('truncated_copy')
    return rows

def inventory(directory, approved, output):
    manifest, dump = retained.validate_export(directory, approved)
    tables = {}
    for table in TABLES:
        text = retained.run(['pg_restore', '--data-only', '--schema=public',
                             '--table=' + table, '--file=-', str(dump)])
        if len(text.encode()) > 64 * 1024 * 1024: raise ValueError('inventory_size_limit')
        tables[table] = copy_rows(text, table)
    objects = {}; archive_count = 0
    for row in tables[TABLES[0]]:
        key = row['object_key']
        if not key.startswith('workspaces/') or '\x00' in key: raise ValueError('unexpected_object_key')
        objects[key] = dict(key=key, kind='transcript-checkpoint',
                            expectedBytes=int(row['ciphertext_bytes']), expectedSha256=row['ciphertext_sha256'])
    for row in tables[TABLES[1]]:
        config = json.loads(row.get('request_config') or '{}')
        archive = config.get('recoveryArchive')
        if not isinstance(archive, dict): continue
        archive_count += 1
        prefix = 'workspaces/' + quote(row['workspace_id'], safe="~()*!.'-") + '/archives/' + quote(archive['id'], safe="~()*!.'-") + '/'
        objects[prefix + 'manifest'] = dict(key=prefix + 'manifest', kind='archive-manifest')
        for chunk in archive.get('manifest', {}).get('chunks', []):
            if not re.fullmatch('[0-9a-f]{64}', chunk): raise ValueError('invalid_archive_chunk')
            objects[prefix + chunk] = dict(key=prefix + chunk, kind='archive-chunk')
    report = dict(version=1, exportSha256=manifest['sha256'], sourceReference=manifest['sourceReference'],
                  sourceFingerprint=approved, tableRows={k: len(v) for k,v in tables.items()},
                  visibleObjectReferences=len(objects), archiveRecords=archive_count,
                  sealedMessageRows=sum(bool(r.get('sealed_content')) for r in tables[TABLES[2]]),
                  attachmentInventoryComplete=False, historicalTranscriptPagesInventoryComplete=False,
                  toolOutputInventoryComplete=False, decryptionVerified=False,
                  sourceConnectionOpened=False, databaseOrContainerStarted=False, sourceWrites=False)
    retained.private_write(output, json.dumps(dict(report=report, objects=list(objects.values())), sort_keys=True))
    return report

def error_code(error):
    code = getattr(error, 'response', {}).get('Error', {}).get('Code', '')
    status = getattr(error, 'response', {}).get('ResponseMetadata', {}).get('HTTPStatusCode')
    if status == 404 or code in ('NoSuchKey', 'NotFound', '404'): return 'not_found'
    if status == 403 or code in ('AccessDenied', '403'): return 'access_denied'
    if status == 401: return 'authentication_failed'
    if code in ('PreconditionFailed', '412'): return 'source_changed'
    return 'object_operation_failed'

def metadata(head):
    return {k: head[k] for k in HTTP_FIELDS if k in head}

def digest_body(body, target, remaining):
    digest = hashlib.sha256(); size = 0
    try:
        while True:
            chunk = body.read(min(1024 * 1024, remaining - size + 1))
            if not chunk: break
            size += len(chunk)
            if size > remaining: raise ValueError('sample_byte_limit')
            digest.update(chunk)
            if target is not None: target.write(chunk)
    finally: body.close()
    return size, digest.hexdigest()

def cleanup(client, bucket, prefix, entries):
    for entry in entries:
        key = entry['destinationKey']
        if not key.startswith(prefix) or key == prefix: raise ValueError('cleanup_scope_invalid')
        client.delete_object(Bucket=bucket, Key=key)
        try: client.head_object(Bucket=bucket, Key=key)
        except Exception as error:
            if error_code(error) != 'not_found': raise ValueError('cleanup_unverified') from None
        else: raise ValueError('cleanup_unverified')

def rehearse(source, destination, source_bucket, destination_bucket, objects, journal_path, limit=5):
    if source_bucket not in SOURCE_BUCKETS: raise ValueError('source_bucket_not_pinned')
    if not 1 <= limit <= 5: raise ValueError('sample_count_limit')
    prefix = 'migration-rehearsal/legacy-a13a32e1c474448f/' + secrets.token_hex(16) + '/'
    journal = dict(version=1, phase='intent', sourceBucket=source_bucket,
                   destinationBucket=destination_bucket, prefix=prefix, entries=[])
    retained.private_write(journal_path, json.dumps(journal))
    def save():
        # Original private file is owned by this process; atomic replacement avoids
        # a partial mapping if the operator dies during a response.
        temp = Path(str(journal_path) + '.next')
        retained.private_write(temp, json.dumps(journal, sort_keys=True))
        os.replace(temp, journal_path)
    total = 0; verified = []; failure = None; skipped = 0
    try:
        for obj in objects:
            if len(verified) == limit: break
            key = obj['key']
            try: head = source.head_object(Bucket=source_bucket, Key=key)
            except Exception as error:
                if error_code(error) == 'not_found': skipped += 1; continue
                raise
            size = head['ContentLength']
            if size <= 0 or size > MAX_BYTES - total: skipped += 1; continue
            if obj.get('expectedBytes', size) != size: raise ValueError('source_reference_size_mismatch')
            with tempfile.TemporaryFile() as spool:
                response = source.get_object(Bucket=source_bucket, Key=key, IfMatch=head['ETag'])
                actual_size, sha = digest_body(response['Body'], spool, MAX_BYTES - total)
                if size != actual_size or response['ETag'] != head['ETag'] or metadata(response) != metadata(head):
                    raise ValueError('source_changed')
                expected = obj.get('expectedSha256')
                if expected and expected not in (sha, base64.urlsafe_b64encode(bytes.fromhex(sha)).decode().rstrip('=')):
                    raise ValueError('source_reference_hash_mismatch')
                total += size
                target = prefix + str(len(journal['entries'])) + '-' + sha
                entry = dict(sourceKey=key, destinationKey=target, bytes=size, sha256=sha,
                             sourceETag=head['ETag'], kind=obj['kind'], verified=False)
                # Persist before PUT; cleanup includes uncertain/lost PUT responses.
                journal['entries'].append(entry); journal['phase'] = 'copying'; save()
                spool.seek(0)
                destination.put_object(Bucket=destination_bucket, Key=target, Body=spool,
                                       ContentLength=size, IfNoneMatch='*', **metadata(head))
                copied_head = destination.head_object(Bucket=destination_bucket, Key=target)
                copied = destination.get_object(Bucket=destination_bucket, Key=target)
                copy_size, copy_sha = digest_body(copied['Body'], None, size)
                if copy_size != size or copy_sha != sha or copied_head['ContentLength'] != size or metadata(copied_head) != metadata(head):
                    raise ValueError('copied_bytes_or_metadata_mismatch')
                current = source.head_object(Bucket=source_bucket, Key=key)
                if current['ETag'] != head['ETag'] or current['ContentLength'] != size or metadata(current) != metadata(head):
                    raise ValueError('source_changed')
                entry['verified'] = True; verified.append(dict(bytes=size, sha256=sha, kind=obj['kind'], metadataEqual=True)); save()
    except Exception as error:
        failure = str(error) if type(error) is ValueError and re.fullmatch('[a-z_]+', str(error)) else error_code(error)
    finally:
        journal['phase'] = 'cleaning'; save()
        cleanup(destination, destination_bucket, prefix, journal['entries'])
        journal['phase'] = 'cleaned'; journal['failure'] = failure; save()
    return dict(mode='bounded-legacy-ciphertext-retention', sampledObjects=len(verified), sampledBytes=total,
                samples=verified, skippedObjects=skipped, failure=failure, copiedObjectsAbsent=True,
                sourceWrites=False, plaintextPrinted=False, decryptionVerified=False,
                completeObjectInventory=False)

def client(env_path):
    import boto3
    from botocore.config import Config
    values = {}
    for line in retained.read_private(env_path, 1024 * 1024).decode().splitlines():
        if line.startswith('CLOUD_TRANSCRIPT_R2_') and '=' in line:
            k,v = line.split('=',1); values[k] = v
    prefix = 'CLOUD_TRANSCRIPT_R2_'
    if not all(values.get(prefix + k) for k in ('ENDPOINT','BUCKET','ACCESS_KEY_ID','SECRET_ACCESS_KEY')):
        raise ValueError('private_r2_configuration_missing')
    endpoint = values[prefix + 'ENDPOINT']
    if not re.fullmatch(r'https://[a-zA-Z0-9.-]+\.r2\.cloudflarestorage\.com/?', endpoint):
        raise ValueError('r2_endpoint_invalid')
    return boto3.client('s3', endpoint_url=endpoint, region_name='auto',
                        aws_access_key_id=values[prefix+'ACCESS_KEY_ID'],
                        aws_secret_access_key=values[prefix+'SECRET_ACCESS_KEY'],
                        config=Config(connect_timeout=10, read_timeout=30, retries={'total_max_attempts':1})), values[prefix+'BUCKET']

def bucket_inventory(source, bucket, output, max_objects=100000):
    """Metadata LIST only; no payload GET or source mutation, including orphans."""
    if bucket not in SOURCE_BUCKETS: raise ValueError('source_bucket_not_pinned')
    objects=[]; token=None; complete=False; started=time.monotonic()
    counts={}; total=0
    for _ in range(100):
        if time.monotonic()-started > 120: break
        args=dict(Bucket=bucket,MaxKeys=min(1000,max_objects-len(objects)))
        if args['MaxKeys'] <= 0: break
        if token: args['ContinuationToken']=token
        page=source.list_objects_v2(**args)
        for obj in page.get('Contents',[]):
            key=obj['Key']
            if '/api-assets/' in key: kind='attachment'
            elif '/archives/' in key: kind='archive-manifest' if key.endswith('/manifest') else 'archive-chunk'
            elif '/sessions/' in key and '/pages/' in key: kind='transcript-page'
            elif '/sessions/' in key and '/generations/' in key: kind='transcript-version'
            else: kind='unclassified'
            counts[kind]=counts.get(kind,0)+1;total+=obj['Size']
            objects.append(dict(key=key,bytes=obj['Size'],etag=obj.get('ETag'),kind=kind))
        if not page.get('IsTruncated'): complete=True;break
        next_token=page.get('NextContinuationToken')
        if not next_token or next_token==token: raise ValueError('invalid_listing_cursor')
        token=next_token
    report=dict(mode='legacy-r2-metadata-inventory',sourceBucket=bucket,objects=len(objects),
                totalStoredBytes=total,kinds=counts,completeBucketListing=complete,
                payloadsRead=0,sourceWrites=False,decryptionVerified=False,
                attachmentSemanticReferencesVerified=False,toolOutputSemanticReferencesVerified=False)
    retained.private_write(output,json.dumps(dict(report=report,objects=objects),sort_keys=True))
    return report

def main():
    p = argparse.ArgumentParser(description=__doc__); sub = p.add_subparsers(dest='mode', required=True)
    i = sub.add_parser('inventory'); i.add_argument('--export-directory',type=Path,required=True)
    i.add_argument('--approve-source-fingerprint',required=True); i.add_argument('--output',type=Path,required=True)
    r = sub.add_parser('copy'); r.add_argument('--inventory',type=Path,required=True)
    r.add_argument('--source-bucket',choices=SOURCE_BUCKETS,required=True)
    r.add_argument('--source-env',type=Path,required=True); r.add_argument('--destination-env',type=Path,required=True)
    r.add_argument('--journal',type=Path,required=True); r.add_argument('--report',type=Path,required=True)
    c = sub.add_parser('cleanup'); c.add_argument('--journal',type=Path,required=True); c.add_argument('--destination-env',type=Path,required=True)
    b = sub.add_parser('inventory-bucket'); b.add_argument('--source-bucket',choices=SOURCE_BUCKETS,required=True)
    b.add_argument('--source-env',type=Path,required=True); b.add_argument('--output',type=Path,required=True)
    a=p.parse_args()
    if a.mode == 'inventory': result=inventory(a.export_directory,a.approve_source_fingerprint,a.output)
    elif a.mode == 'inventory-bucket':
        source,_=client(a.source_env);result=bucket_inventory(source,a.source_bucket,a.output)
    elif a.mode == 'cleanup':
        journal=json.loads(retained.read_private(a.journal,1024*1024)); dest,bucket=client(a.destination_env)
        if bucket != journal['destinationBucket'] or not re.fullmatch(r'migration-rehearsal/legacy-a13a32e1c474448f/[a-f0-9]{32}/',journal['prefix']):raise ValueError('cleanup_scope_invalid')
        cleanup(dest,bucket,journal['prefix'],journal['entries']); result={'copiedObjectsAbsent':True}
    else:
        data=json.loads(retained.read_private(a.inventory,16*1024*1024))
        source,_=client(a.source_env); dest,bucket=client(a.destination_env)
        result=rehearse(source,dest,a.source_bucket,bucket,data['objects'],a.journal)
        retained.private_write(a.report,json.dumps(result,sort_keys=True,indent=2))
    print(json.dumps(result,sort_keys=True))

if __name__ == '__main__':
    try: main()
    except Exception as error:
        code=str(error) if type(error) is ValueError and re.fullmatch('[a-z_]+',str(error)) else 'private_rehearsal_failed'
        print(json.dumps({'failure':code})); raise SystemExit(1)
