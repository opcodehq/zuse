#!/usr/bin/env python3
"""Fenced, idempotent environment reapplication on an explicitly replaced disk.

Called only by the private resident Supervisor. No commands or repositories are
changed here. The existing guest reapplies its configured environment recipe
because the previous computer's completed-setup receipt is retained separately.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat

FIELDS={'version','action','operationId','archiveOperationId','workspaceId','ownerId','generation','storageIncarnationId','archiveSha256'}
SAFE=re.compile(r'[A-Za-z0-9_.:-]{1,160}\Z')
DIGEST=re.compile(r'[a-f0-9]{64}\Z')
MARKER='cloud-runtime-supervisor-recreation.json'

def canonical(value):return json.dumps(value,sort_keys=True,separators=(',',':')).encode()

def private_json(path,limit,require):
    fd=os.open(path,os.O_RDONLY|os.O_CLOEXEC|os.O_NOFOLLOW)
    try:
        info=os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_uid==os.getuid() and not info.st_mode&0o077 and info.st_size<=limit,'recreation_private_file_invalid')
        with os.fdopen(fd,'rb',closefd=False) as source:return json.load(source)
    finally:os.close(fd)

def sync_directory(path):
    fd=os.open(path,os.O_DIRECTORY|os.O_CLOEXEC|os.O_NOFOLLOW)
    try:os.fsync(fd)
    finally:os.close(fd)

def prepare(supervisor,request,*,require,atomic,load_private):
    require(isinstance(request,dict) and set(request)==FIELDS and request.get('version')==1 and request.get('action')=='prepare-recreation','recreation_request_invalid')
    for field in ('operationId','archiveOperationId','workspaceId','ownerId','storageIncarnationId'):
        require(isinstance(request[field],str) and SAFE.fullmatch(request[field]),'recreation_binding_invalid')
    require(type(request['generation']) is int and 1<=request['generation']<=2**63-1,'recreation_generation_invalid')
    require(isinstance(request['archiveSha256'],str) and DIGEST.fullmatch(request['archiveSha256']),'recreation_archive_invalid')
    identity={key:request[key] for key in ('workspaceId','generation','storageIncarnationId')}
    require(supervisor.recovery_binding==identity,'recreation_supervisor_binding_mismatch')
    if supervisor.record:
        require(all(supervisor.record['identity'].get(key)==value for key,value in identity.items()),'recreation_supervisor_binding_mismatch')
    marker=supervisor.data/MARKER
    fingerprint=hashlib.sha256(canonical(request)).hexdigest()
    prior=load_private(marker) if marker.exists() else None
    if prior:
        require(prior.get('fingerprint')==fingerprint and prior.get('request')==request,'recreation_operation_conflict')
        if prior.get('phase')=='ready':return prior['receipt']
    supervisor.reap()
    require(not supervisor.alive() and not any(child.poll() is None for child in supervisor.recoveries.values()),'recreation_runtime_not_idle')
    require(not any(record.get('phase')!='ready' for record in supervisor.repairs.values()),'recreation_runtime_repair_in_progress')
    # Respect the same exclusive lock used by both guest and recovery helper.
    lock=os.open(supervisor.data/'.workspace-runtime.lock',os.O_RDWR|os.O_CREAT|os.O_CLOEXEC|os.O_NOFOLLOW,0o600)
    try:
        info=os.fstat(lock)
        require(stat.S_ISREG(info.st_mode) and info.st_uid==os.getuid() and not info.st_mode&0o077,'recreation_data_lock_invalid')
        try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        except BlockingIOError:require(False,'recreation_data_busy')
        restored=private_json(supervisor.data/'rust-recovery-restored.json',65536,require)
        binding=restored.get('binding',{});storage=binding.get('rustStorage',{})
        require(binding.get('operationId')==request['archiveOperationId'] and binding.get('workspaceId')==request['workspaceId'] and storage.get('accountId')==request['ownerId'] and storage.get('storageIncarnationId')==request['storageIncarnationId'] and restored.get('archiveSha256')==request['archiveSha256'],'recreation_restored_archive_mismatch')
        root=supervisor.data/'.runtime-recreation'
        if not root.exists():root.mkdir(mode=0o700)
        require(not root.is_symlink() and stat.S_ISDIR(root.lstat().st_mode) and root.lstat().st_uid==os.getuid() and not root.lstat().st_mode&0o077,'recreation_receipt_directory_invalid')
        old=root/'environment.json';ready=supervisor.data/'environment-ready.json'
        if ready.exists() or ready.is_symlink():
            environment=private_json(ready,2*1024*1024,require)
            require(environment.get('workspace')==request['workspaceId'] and environment.get('incarnation')==request['storageIncarnationId'] and environment.get('checkout')==str(supervisor.checkout),'recreation_environment_binding_mismatch')
        else:environment=None
        receipt=dict(version=1,status='prepared',operationId=request['operationId'],archiveOperationId=request['archiveOperationId'],workspaceId=request['workspaceId'],ownerId=request['ownerId'],generation=request['generation'],storageIncarnationId=request['storageIncarnationId'],archiveSha256=request['archiveSha256'],environmentReapply=True)
        if prior is None:
            prior=dict(version=1,phase='preparing',fingerprint=fingerprint,request=request,hadEnvironment=environment is not None,receipt=receipt)
            atomic(marker,prior)
        if environment is not None:
            if old.exists():
                require(private_json(old,2*1024*1024,require)==environment,'recreation_environment_receipt_conflict')
                ready.unlink()
            else:os.replace(ready,old)
            sync_directory(root);sync_directory(supervisor.data)
        elif prior.get('hadEnvironment'):
            require(old.is_file() and not old.is_symlink(),'recreation_environment_receipt_missing')
            private_json(old,2*1024*1024,require)
        prior['phase']='ready';atomic(marker,prior)
        return prior['receipt']
    finally:os.close(lock)
