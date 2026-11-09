#!/usr/bin/env python3
"""Resident, account-free boot owner and bounded private runtime handoff.

The resident process is the provider's boot command, never a one-shot exec child.
Only workspace/generation/disk metadata and an existing private token FILE path
cross IPC; prompts and credential values never enter launch requests or receipts.
"""
import argparse
import ctypes
import errno
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pwd
import re
import signal
import socket
import stat
import struct
import subprocess
import sys
import time
import urllib.parse
import uuid

VERSION=1
LIMIT=65536
SAFE=re.compile(r'[A-Za-z0-9_.:-]{1,160}\Z')
INHERIT=('PATH','HOME','USER','LOGNAME','LANG','LC_ALL','SHELL','TMPDIR',
         'ZUSE_HARNESS_ROOT','ZUSE_HARNESS_CHANNEL','ZUSE_RUNTIME_BINARY')
FIELDS={'version','action','operationId','workspaceId','generation','storageIncarnationId','apiUrl','bootTokenFile','retained'}

class Rejected(Exception):pass

def require(value,code):
    if not value:raise Rejected(code)

def canonical(value):return json.dumps(value,sort_keys=True,separators=(',',':')).encode()

def atomic(path,value):
    temporary=path.parent/('.runtime-write-supervisor-'+uuid.uuid4().hex)
    fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_CLOEXEC,0o600)
    try:
        with os.fdopen(fd,'wb') as output:output.write(canonical(value)+b'\n');output.flush();os.fsync(output.fileno())
        os.replace(temporary,path)
        directory=os.open(path.parent,os.O_DIRECTORY|os.O_CLOEXEC)
        try:os.fsync(directory)
        finally:os.close(directory)
    finally:temporary.unlink(missing_ok=True)

def private_directory(path):
    require(path.is_absolute(),'supervisor_directory_invalid')
    for parent in path.parents:
        if parent.exists():require(not parent.is_symlink(),'supervisor_directory_symlink')
    path.mkdir(mode=0o700,parents=True,exist_ok=True)
    info=path.lstat();require(stat.S_ISDIR(info.st_mode) and info.st_uid==os.getuid(),'supervisor_directory_owner')
    require(stat.S_IMODE(info.st_mode)&0o077==0,'supervisor_directory_not_private')

def load_private(path):
    fd=os.open(path,os.O_RDONLY|os.O_CLOEXEC|os.O_NOFOLLOW)
    try:
        info=os.fstat(fd)
        require(stat.S_ISREG(info.st_mode) and info.st_uid==os.getuid() and not info.st_mode&0o077 and info.st_size<=LIMIT,'supervisor_private_file_invalid')
        with os.fdopen(fd,'rb',closefd=False) as source:return json.load(source)
    finally:os.close(fd)

def text(value,code):
    require(isinstance(value,str) and 0<len(value)<=512 and all(ord(c)>=32 for c in value),code)
    return value

def validated(request,status_root):
    require(isinstance(request,dict) and not(set(request)-FIELDS) and request.get('version')==VERSION,'supervisor_request_invalid')
    require(request.get('action')=='launch','supervisor_action_invalid')
    operation=request.get('operationId');require(isinstance(operation,str) and SAFE.fullmatch(operation),'supervisor_operation_invalid')
    generation=request.get('generation');require(type(generation) is int and 1<=generation<=2**63-1,'supervisor_generation_invalid')
    require(type(request.get('retained')) is bool,'supervisor_mode_invalid')
    workspace=text(request.get('workspaceId'),'supervisor_workspace_invalid')
    disk=text(request.get('storageIncarnationId'),'supervisor_disk_invalid')
    api=text(request.get('apiUrl'),'supervisor_api_invalid').rstrip('/')
    parsed=urllib.parse.urlsplit(api)
    require(parsed.scheme=='https' or (parsed.scheme=='http' and parsed.hostname in ('127.0.0.1','localhost','::1')),'supervisor_api_invalid')
    require(parsed.hostname and not parsed.username and not parsed.password and not parsed.query and not parsed.fragment,'supervisor_api_invalid')
    token=request.get('bootTokenFile')
    if request['retained']:require(token is None,'supervisor_retained_token_rejected')
    else:
        require(token==str(status_root/('rust-boot-'+operation)),'supervisor_boot_path_invalid')

    return dict(version=VERSION,workspaceId=workspace,generation=generation,storageIncarnationId=disk,apiUrl=api),operation

def child_session(parent):
    # Keep the child in this resident namespace, but not in a transient caller's
    # process group. If the supervisor dies, its child must not become an orphan.
    os.setsid()
    libc=ctypes.CDLL(None)
    libc.prctl(1,signal.SIGTERM,0,0,0)
    if os.getppid()!=parent:os.kill(os.getpid(),signal.SIGTERM)

class Supervisor:
    def __init__(self,data,checkout,status_root,launcher,binary,recovery_root=None,recovery_launcher=None):
        self.data=data;self.checkout=checkout;self.status_root=status_root;self.launcher=launcher;self.binary=binary
        private_directory(data)
        self.ipc=data/'.runtime-supervision';private_directory(self.ipc)
        self.identity_path=data/'cloud-runtime-supervision.json'
        self.record=load_private(self.identity_path) if self.identity_path.exists() else None
        self.recovery_binding_path=data/'cloud-runtime-supervisor-recovery.json'
        self.recovery_binding=load_private(self.recovery_binding_path) if self.recovery_binding_path.exists() else None
        self.recovery_root=recovery_root or Path('/var/lib/zuse/recovery')
        self.recovery_launcher=recovery_launcher or Path('/usr/local/lib/zuse/launch-rust-recovery')
        self.child=None;self.recoveries={};self.cancelled={};self.nonce=uuid.uuid4().hex;self.stopping=False
        self.repair_module=None;self.repair_launch=False
        module_path=Path(__file__).with_name('runtime-repair.py')
        if module_path.is_file() and not module_path.is_symlink():
            spec=importlib.util.spec_from_file_location('zuse_runtime_repair',module_path)
            module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
            if module.enable_subreaper():self.repair_module=module
        self.recreation_module=None
        module_path=Path(__file__).with_name('runtime-recreation.py')
        if module_path.is_file() and not module_path.is_symlink():
            spec=importlib.util.spec_from_file_location('zuse_runtime_recreation',module_path)
            module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
            self.recreation_module=module
        self.repairs_path=data/'cloud-runtime-supervisor-repairs.json'
        self.repairs=load_private(self.repairs_path) if self.repairs_path.exists() else {}
        self.socket_path=self.ipc/'control.sock'
        self.base_env={key:value for key,value in os.environ.items() if key in INHERIT}
        self.base_env.update(HOME=str(data.parent),USER=pwd.getpwuid(os.getuid()).pw_name,
            ZUSE_USER_DATA=str(data),ZUSE_CLOUD_WORKSPACE_ROOT=str(checkout),
            ZUSE_WORKSPACE_RUNTIME_STATUS_DIR=str(status_root),ZUSE_RUNTIME_BINARY=str(binary))
    def reap(self):
        while True:
            try:pid,status=os.waitpid(-1,os.WNOHANG)
            except ChildProcessError:return
            if not pid:return
            if self.child and self.child.pid==pid:self.child.returncode=os.waitstatus_to_exitcode(status)
            for child in self.recoveries.values():
                if child.pid==pid:child.returncode=os.waitstatus_to_exitcode(status)
    def alive(self):return self.child is not None and self.child.poll() is None
    def ping(self):
        self.reap()
        return dict(version=VERSION,publicationStopVersion=1,repairVersion=1 if self.repair_module else None,recreationVersion=1 if self.recreation_module else None,bootNonce=self.nonce,residentPid=os.getpid(),bound=self.record is not None,**({'binding':{k:self.record['identity'].get(k) for k in ('version','workspaceId','generation','storageIncarnationId','apiUrl')}} if self.record else {}),childAlive=self.alive(),recoveryAlive=any(child.poll() is None for child in self.recoveries.values()),childPid=self.child.pid if self.alive() else None)
    def publication_stop(self,request):
        # Only clean, credential-free builders may end this resident. This is
        # deliberately separate from retained-task pause/repair/recovery.
        require(isinstance(request,dict) and set(request)=={'version','action','operationId','expectedBootNonce'} and request['version']==VERSION,'publication_stop_frame_invalid')
        require(isinstance(request['operationId'],str) and SAFE.fullmatch(request['operationId']) and request['expectedBootNonce']==self.nonce,'publication_stop_identity_mismatch')
        self.reap()
        require(self.record is None and self.recovery_binding is None and not self.repairs and not self.alive() and not any(child.poll() is None for child in self.recoveries.values()) and not self.cancelled,'publication_stop_runtime_bound')
        require(os.getpid()!=1,'publication_stop_primary_owner_unsupported')
        for root in (self.checkout,self.status_root,self.recovery_root):
            require(root.is_dir() and not root.is_symlink() and not any(root.iterdir()),'publication_stop_runtime_data_present')
        require(set(path.name for path in self.data.iterdir())=={'.runtime-supervision'},'publication_stop_runtime_data_present')
        require(not Path('/proc/self/task/'+str(os.getpid())+'/children').read_text().split(),'publication_stop_live_descendants')
        require(set(path.name for path in self.ipc.iterdir())=={'supervisor.lock','supervisor.json','control.sock'},'publication_stop_unknown_ipc')
        for name,socket_file in [('supervisor.lock',False),('supervisor.json',False),('control.sock',True)]:
            meta=(self.ipc/name).lstat()
            require(meta.st_uid==os.getuid() and meta.st_nlink==1 and stat.S_IMODE(meta.st_mode)==0o600 and (stat.S_ISSOCK(meta.st_mode) if socket_file else stat.S_ISREG(meta.st_mode) and meta.st_nlink==1),'publication_stop_ipc_owner_invalid')
        saved=load_private(self.ipc/'supervisor.json')
        require(set(saved)=={'version','bootNonce','residentPid'} and saved.get('version')==VERSION and saved.get('residentPid')==os.getpid() and saved.get('bootNonce')==self.nonce,'publication_stop_resident_record_mismatch')
        self.stopping=True
        return dict(version=VERSION,action='publication-stop',operationId=request['operationId'],residentPid=os.getpid(),bootNonce=self.nonce,state='idle-stopping')
    def launch(self,request):
        self.reap()
        require(self.repair_launch or not any(r.get('phase')!='ready' for r in self.repairs.values()),'runtime_supervisor_repair_in_progress')
        require(not any(child.poll() is None for child in self.recoveries.values()),'runtime_supervisor_recovery_busy')
        identity,operation=validated(request,self.status_root)
        if self.record:require(self.record['identity']==identity,'runtime_supervisor_fenced')
        if self.recovery_binding:require(all(identity.get(key)==value for key,value in self.recovery_binding.items()),'runtime_supervisor_recovery_fenced')
        fingerprint=hashlib.sha256(canonical(request)).hexdigest()
        receipts=self.record.get('receipts',{}) if self.record else {}
        if operation in receipts:require(receipts[operation]==fingerprint,'runtime_supervisor_operation_conflict')
        if self.alive() and operation in receipts:
            # A local ACK confirms delivery, not successful remote authority or
            # grants. Re-deliver this same durable operation only while the
            # owned core is still parked; active children remain idempotent.
            parked=False
            if request['retained']:
                memory=self.data/'.runtime-memory';wake=memory/'wake.sock'
                if os.path.lexists(wake):
                    directory=memory.lstat();meta=wake.lstat()
                    require(stat.S_ISDIR(directory.st_mode) and directory.st_uid==os.getuid() and not directory.st_mode&0o077,'runtime_supervisor_memory_directory_invalid')
                    require(stat.S_ISSOCK(meta.st_mode) and meta.st_uid==os.getuid() and meta.st_nlink==1 and not meta.st_mode&0o077,'runtime_supervisor_memory_socket_invalid')
                    parked=True
            if not parked:return dict(version=VERSION,status='accepted',operationId=operation,childPid=self.child.pid)
        if self.alive():
            # A memory-preserving resume wakes the same core. The guest's own
            # signed lifecycle protocol remains the authority to reopen execution.
            if request['retained']:
                result=subprocess.run([str(self.binary),'--workspace-id',identity['workspaceId'],
                    '--generation',str(identity['generation']),'--storage-incarnation-id',identity['storageIncarnationId'],
                    '--data-dir',str(self.data),'notify-resume','--operation-id',operation],
                    env=self.base_env,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=8)
                require(result.returncode==0,'runtime_supervisor_resume_unconfirmed')
            else:require(operation in receipts,'runtime_supervisor_runtime_busy')
        else:
            # A dead Rust parent does not imply its setsid/native/tool children
            # stopped. Fence every owned descendant before reopening its WAL.
            # Live-child/idempotent and memory-resume paths above never retire it.
            if self.record is not None or self.child is not None:
                require(self.repair_module is not None,'runtime_supervisor_cold_recovery_unavailable')
                self.repair_module.retire_owned(os.getpid())
            # Persist original intent before spawning. A repeated delivery never
            # supplies/replays a prompt. The Rust journal owns command recovery.
            retained=request['retained'] or (self.data/'cloud-runtime-identity.json').is_file()
            if retained:require((self.data/'cloud-runtime-identity.json').is_file(),'supervisor_retained_identity_missing')
            else:
                info=Path(request['bootTokenFile']).lstat()
                require(stat.S_ISREG(info.st_mode) and info.st_uid==os.getuid() and not info.st_mode&0o077,'supervisor_boot_file_invalid')
            receipts[operation]=fingerprint
            self.record=dict(version=VERSION,identity=identity,receipts=dict(list(receipts.items())[-32:]))
            atomic(self.identity_path,self.record)
            env=dict(self.base_env,ZUSE_CLOUD_WORKSPACE_ID=identity['workspaceId'],ZUSE_API_URL=identity['apiUrl'],
                ZUSE_RUNTIME_GENERATION=str(identity['generation']),ZUSE_STORAGE_INCARNATION_ID=identity['storageIncarnationId'],
                ZUSE_RUNTIME_OPERATION_ID=operation,ZUSE_RUNTIME_RETAINED='true' if retained else 'false')
            if not retained:env['ZUSE_RUNTIME_BOOT_TOKEN_FILE']=request['bootTokenFile']
            self.status_root.mkdir(mode=0o700,parents=True,exist_ok=True)
            log=os.open(self.status_root/'rust-runtime.log',os.O_WRONLY|os.O_APPEND|os.O_CREAT|os.O_CLOEXEC|os.O_NOFOLLOW,0o600)
            parent=os.getpid()
            try:self.child=subprocess.Popen([str(self.launcher),'--foreground'],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log,preexec_fn=lambda:child_session(parent),close_fds=True)
            finally:os.close(log)
        receipts[operation]=fingerprint
        self.record=dict(version=VERSION,identity=identity,receipts=dict(list(receipts.items())[-32:]))
        atomic(self.identity_path,self.record)
        return dict(version=VERSION,status='accepted',operationId=operation,childPid=self.child.pid)
    def recovery(self,request):
        require(set(request)=={'version','action','operationId'} and request['version']==VERSION,'supervisor_recovery_request_invalid')
        operation=request['operationId'];require(isinstance(operation,str) and SAFE.fullmatch(operation),'supervisor_operation_invalid')
        self.reap();directory=self.recovery_root/operation;private_directory(directory)
        if request['action']=='cancel-recovery':
            child=self.recoveries.get(operation)
            (directory/'cancelled').touch(mode=0o600)
            if child and child.poll() is None:
                try:os.killpg(child.pid,signal.SIGTERM)
                except ProcessLookupError:pass
                self.cancelled.setdefault(operation,time.monotonic())
            return dict(version=VERSION,status='accepted',operationId=operation)
        require(not self.alive(),'runtime_supervisor_runtime_not_quiesced')
        config=load_private(directory/'config.json');binding=config.get('supervisorBinding')
        require(isinstance(binding,dict) and set(binding)=={'workspaceId','generation','storageIncarnationId'},'supervisor_recovery_binding_required')
        require(config.get('operationId')==operation and config.get('workspaceId')==binding['workspaceId'],'supervisor_recovery_binding_invalid')
        require(type(binding['generation']) is int and binding['generation']>0 and isinstance(binding['storageIncarnationId'],str),'supervisor_recovery_binding_invalid')
        if self.record:
            require(all(self.record['identity'].get(key)==value for key,value in binding.items()),'runtime_supervisor_fenced')
        else:require(config.get('mode')=='restore','runtime_supervisor_recovery_unbound')
        require(config.get('home')==str(self.data.parent) and config.get('repositoriesPath')==str(self.checkout),'supervisor_recovery_layout_invalid')
        self.recovery_binding=binding;atomic(self.recovery_binding_path,binding)
        child=self.recoveries.get(operation)
        if child is None or child.poll() is not None:
            require(not(directory/'cancelled').exists(),'supervisor_recovery_cancelled')
            env=dict(self.base_env,ZUSE_RECOVERY_STATUS_ROOT=str(self.recovery_root))
            log=os.open(directory/'helper.log',os.O_WRONLY|os.O_APPEND|os.O_CREAT|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
            parent=os.getpid()
            try:child=subprocess.Popen([str(self.recovery_launcher),operation,'--foreground'],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log,preexec_fn=lambda:child_session(parent),close_fds=True)
            finally:os.close(log)
            self.recoveries[operation]=child
        return dict(version=VERSION,status='accepted',operationId=operation,childPid=child.pid)
    def repair(self,request):
        require(self.repair_module is not None,'runtime_repair_unavailable')
        action=request.get('action');extra={'commandIds','resumeOperationId'} if action=='repair-start' else set()
        allowed={'version','action','operationId','workspaceId','generation','storageIncarnationId','apiUrl'}|extra
        require(set(request)==allowed,'runtime_repair_request_invalid')
        launch=dict(request);launch.pop('commandIds',None);launch.pop('resumeOperationId',None)
        launch.update(action='launch',retained=True)
        identity,operation=validated(launch,self.status_root)
        require(self.record and self.record['identity']==identity,'runtime_repair_identity_fenced')
        require(not any(child.poll() is None for child in self.recoveries.values()),'runtime_repair_recovery_busy')
        self.reap();record=self.repairs.get(operation)
        if record:require(record['identity']==identity,'runtime_repair_identity_fenced')
        if record is None:
            require(action=='repair-stop','runtime_repair_operation_missing')
            require(not any(r.get('phase')!='ready' for r in self.repairs.values()),'runtime_repair_in_progress')
            # Keep one full active receipt; old completed operations remain small
            # immutable no-op receipts so lost stop replies cannot kill a new PID.
            self.repairs={k:{key:v[key] for key in ('identity','phase','originalChildPid','childPid','graceful','resumeOperationId','startFingerprint','supervisorNonce') if key in v} for k,v in list(self.repairs.items())[-7:]}
            alive=self.alive();original=self.child.pid if self.child is not None else None
            record=dict(identity=identity,phase='stopping' if alive else 'stopped',originalChildPid=original,childPid=None,graceful=False,startedAt=time.time(),supervisorNonce=self.nonce)
            self.repairs[operation]=record;atomic(self.repairs_path,self.repairs)
            if alive:
                try:self.child.send_signal(signal.SIGINT)
                except ProcessLookupError:pass
        if record['phase']=='stopping':
            if not self.alive():
                record.update(phase='stopped',graceful=self.child is not None and self.child.returncode==0)
                atomic(self.repairs_path,self.repairs)
            elif action=='repair-stop' and time.time()-record['startedAt']>=10:
                record['forced']=True;atomic(self.repairs_path,self.repairs)
                self.repair_module.retire_owned(os.getpid())
                self.reap();record.update(phase='stopped',graceful=False);atomic(self.repairs_path,self.repairs)
        if record['phase']=='starting':
            require(record.get('supervisorNonce')==self.nonce and self.alive() and self.child.pid!=record.get('originalChildPid'),'runtime_repair_launch_outcome_uncertain')
            record.update(phase='ready',childPid=self.child.pid);atomic(self.repairs_path,self.repairs)
        if action=='repair-start' and record['phase']!='ready':
            require(record.get('supervisorNonce')==self.nonce,'runtime_repair_supervisor_incarnation_uncertain')
            resume=request['resumeOperationId'];commands=request['commandIds']
            require(isinstance(resume,str) and SAFE.fullmatch(resume),'runtime_repair_resume_invalid')
            require(isinstance(commands,list) and len(commands)<=128 and len(set(commands))==len(commands) and all(isinstance(c,str) and SAFE.fullmatch(c) for c in commands),'runtime_repair_commands_invalid')
            fingerprint=hashlib.sha256(canonical(request)).hexdigest()
            require(record.get('startFingerprint',fingerprint)==fingerprint,'runtime_repair_operation_conflict')
            if record['phase']=='starting' and self.alive() and self.child.pid!=record.get('originalChildPid'):
                record.update(phase='ready',childPid=self.child.pid);atomic(self.repairs_path,self.repairs)
            else:
                require(record['phase'] in ('stopped','starting'),'runtime_repair_shutdown_pending')
                # Root tools/setsid grandchildren must be retired, not merely the
                # original Rust PID. No new process may start before this fence.
                self.repair_module.retire_owned(os.getpid())
                lock=os.open(self.data/'.workspace-runtime.lock',os.O_RDWR|os.O_NOFOLLOW|os.O_CLOEXEC)
                try:
                    meta=os.fstat(lock);require(stat.S_ISREG(meta.st_mode) and meta.st_uid==os.getuid() and not meta.st_mode&0o077,'runtime_repair_lock_invalid')
                    fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
                    recovery=self.repair_module.reconcile(self.data,identity,commands)
                    wake=self.data/'.runtime-memory/wake.sock'
                    if wake.exists():
                        meta=wake.lstat();require(stat.S_ISSOCK(meta.st_mode) and meta.st_uid==os.getuid() and not meta.st_mode&0o077,'runtime_repair_memory_socket_invalid');wake.unlink()
                    record.update(phase='starting',startFingerprint=fingerprint,resumeOperationId=resume,recovery=recovery)
                    atomic(self.repairs_path,self.repairs)
                finally:os.close(lock)
                self.repair_launch=True
                try:started=self.launch(dict(launch,operationId=resume))
                finally:self.repair_launch=False
                record.update(phase='ready',childPid=started['childPid']);atomic(self.repairs_path,self.repairs)
        if action=='repair-start' and record['phase']=='ready' and record.get('startFingerprint'):
            require(record['startFingerprint']==hashlib.sha256(canonical(request)).hexdigest(),'runtime_repair_operation_conflict')
        return dict(version=VERSION,operationId=operation,workspaceId=identity['workspaceId'],generation=identity['generation'],storageIncarnationId=identity['storageIncarnationId'],phase=record['phase'],originalChildPid=record.get('originalChildPid'),childPid=record.get('childPid'),graceful=record.get('graceful',False),**({'recovery':record['recovery']} if 'recovery' in record else {}))
    def handle(self,request):
        if request=={'version':VERSION,'action':'ping'}:return self.ping()
        if isinstance(request,dict) and request.get('action')=='publication-stop':return self.publication_stop(request)
        if isinstance(request,dict) and request.get('action')=='prepare-recreation':
            require(self.recreation_module is not None,'runtime_recreation_unavailable')
            return self.recreation_module.prepare(self,request,require=require,atomic=atomic,load_private=load_private)
        if isinstance(request,dict) and request.get('action') in ('recover','cancel-recovery'):return self.recovery(request)
        if isinstance(request,dict) and request.get('action') in ('repair-stop','repair-status','repair-start'):return self.repair(request)
        return self.launch(request)
    def stale_socket(self):
        # The held visible flock is necessary but does not authorize deleting a
        # replacement listener which owns a different lock inode.
        if not os.path.lexists(self.socket_path):return
        before=self.socket_path.lstat()
        require(stat.S_ISSOCK(before.st_mode) and before.st_uid==os.getuid() and stat.S_IMODE(before.st_mode)==0o600,'supervisor_socket_invalid')
        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as probe:
            probe.settimeout(.25)
            try:probe.connect(str(self.socket_path))
            except OSError as error:require(error.errno in (errno.ECONNREFUSED,errno.ENOENT),'supervisor_socket_uncertain')
            else:raise Rejected('supervisor_socket_live')
        after=self.socket_path.lstat();require((before.st_dev,before.st_ino)==(after.st_dev,after.st_ino),'supervisor_socket_changed')
        self.socket_path.unlink()
    def remove_owned_socket(self,parent_identity,socket_identity):
        try:
            parent=self.ipc.lstat();current=self.socket_path.lstat()
            if stat.S_ISDIR(parent.st_mode) and (parent.st_dev,parent.st_ino)==parent_identity and stat.S_ISSOCK(current.st_mode) and (current.st_dev,current.st_ino)==socket_identity:self.socket_path.unlink()
        except FileNotFoundError:pass
    def run(self):
        lock=os.open(self.ipc/'supervisor.lock',os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW|os.O_CLOEXEC,0o600)
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        self.stale_socket()
        server=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);server.bind(str(self.socket_path));os.chmod(self.socket_path,0o600);server.listen(8);server.settimeout(1)
        parent=self.ipc.lstat();owned_socket=self.socket_path.lstat();parent_identity=(parent.st_dev,parent.st_ino);socket_identity=(owned_socket.st_dev,owned_socket.st_ino)
        atomic(self.ipc/'supervisor.json',dict(version=VERSION,bootNonce=self.nonce,residentPid=os.getpid()))
        def stop(*_):self.stopping=True
        signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
        try:
            while not self.stopping:
                self.reap()
                for operation,started in list(self.cancelled.items()):
                    child=self.recoveries.get(operation)
                    if child and child.poll() is None and time.monotonic()-started>5:
                        try:os.killpg(child.pid,signal.SIGKILL)
                        except ProcessLookupError:pass
                    if child is None or child.poll() is not None:self.cancelled.pop(operation,None)
                try:connection,_=server.accept()
                except socket.timeout:continue
                with connection:
                    connection.settimeout(10)
                    try:
                        _,uid,_=struct.unpack('3i',connection.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
                        require(uid in (0,os.getuid()),'supervisor_peer_rejected')
                        payload=bytearray()
                        while b'\n' not in payload:
                            block=connection.recv(min(4096,LIMIT+1-len(payload)))
                            require(block and len(payload)+len(block)<=LIMIT,'supervisor_frame_invalid');payload.extend(block)
                        request=json.loads(payload.split(b'\n',1)[0]);response=self.handle(request)
                    except Rejected as error:response={'version':VERSION,'error':str(error)}
                    except Exception:response={'version':VERSION,'error':'runtime_supervisor_request_failed'}
                    try:connection.sendall(canonical(response)+b'\n')
                    except OSError:pass
        finally:
            server.close();self.remove_owned_socket(parent_identity,socket_identity)
            children=([self.child] if self.child else [])+list(self.recoveries.values())
            for child in children:
                if child.poll() is None:
                    try:os.killpg(child.pid,signal.SIGINT)
                    except ProcessLookupError:pass
            deadline=time.monotonic()+5
            while time.monotonic()<deadline and any(child.poll() is None for child in children):time.sleep(.05)
            for child in children:
                if child.poll() is None:
                    try:os.killpg(child.pid,signal.SIGKILL)
                    except ProcessLookupError:pass
                child.wait()
            os.close(lock)

def rpc(data,request,connect_timeout=3):
    address=data/'.runtime-supervision/control.sock'
    deadline=time.monotonic()+connect_timeout
    while True:
        client=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
        try:
            client.settimeout(max(.01,min(10,deadline-time.monotonic())))
            client.connect(str(address))
            break
        except OSError as error:
            client.close()
            # Provider command-ready is earlier than native service-ready. It is
            # safe to wait BEFORE sending anything, never after uncertain send.
            if error.errno not in (errno.ENOENT,errno.ECONNREFUSED) or time.monotonic()>=deadline:raise
            time.sleep(min(.05,max(0,deadline-time.monotonic())))
    with client:
        client.settimeout(10);client.sendall(canonical(request)+b'\n')
        response=bytearray()
        while b'\n' not in response:
            block=client.recv(min(4096,LIMIT+1-len(response)))
            require(block and len(response)+len(block)<=LIMIT,'supervisor_response_invalid');response.extend(block)
    value=json.loads(response.split(b'\n',1)[0]);require('error' not in value,value.get('error','runtime_supervisor_rejected'));return value

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version',action='store_true')
    parser.add_argument('--data-dir',type=Path,default=Path(os.environ.get('ZUSE_USER_DATA','/home/zuse/.zuse-data')))
    parser.add_argument('--workspace-root',type=Path,default=Path(os.environ.get('ZUSE_CLOUD_WORKSPACE_ROOT','/home/zuse/workspace')))
    parser.add_argument('--status-dir',type=Path,default=Path(os.environ.get('ZUSE_WORKSPACE_RUNTIME_STATUS_DIR','/var/lib/zuse/workspace')))
    parser.add_argument('--launcher',type=Path,default=Path('/usr/local/lib/zuse/launch-rust-runtime'))
    parser.add_argument('--binary',type=Path,default=Path(os.environ.get('ZUSE_RUNTIME_BINARY','/usr/local/bin/zuse-cloud-runtime')))
    parser.add_argument('--recovery-root',type=Path,default=Path(os.environ.get('ZUSE_RECOVERY_STATUS_ROOT','/var/lib/zuse/recovery')))
    parser.add_argument('--recovery-launcher',type=Path,default=Path('/usr/local/lib/zuse/launch-rust-recovery'))
    parser.add_argument('--operation-id')
    parser.add_argument('action',nargs='?',choices=('serve','launch','ping','recover','cancel-recovery','rpc'))
    args=parser.parse_args()
    if args.version:print('zuse-runtime-supervisor 1');return
    require(args.action is not None,'supervisor_action_required')
    if args.action=='serve':
        if os.getuid()==0:
            account=pwd.getpwnam('zuse');os.initgroups(account.pw_name,account.pw_gid);os.setgid(account.pw_gid);os.setuid(account.pw_uid)
        os.umask(0o077)
        Supervisor(args.data_dir,args.workspace_root,args.status_dir,args.launcher,args.binary,args.recovery_root,args.recovery_launcher).run();return
    if args.action=='rpc':
        payload=sys.stdin.buffer.read(LIMIT+1);require(len(payload)<=LIMIT,'supervisor_frame_invalid')
        print(json.dumps(rpc(args.data_dir,json.loads(payload))));return
    if args.action=='ping':print(json.dumps(rpc(args.data_dir,{'version':VERSION,'action':'ping'})));return
    if args.action in ('recover','cancel-recovery'):
        rpc(args.data_dir,dict(version=VERSION,action=args.action,operationId=args.operation_id));return
    request=dict(version=VERSION,action='launch',operationId=os.environ.get('ZUSE_RUNTIME_RESUME_OPERATION_ID') or os.environ.get('ZUSE_RUNTIME_OPERATION_ID'),
        workspaceId=os.environ.get('ZUSE_CLOUD_WORKSPACE_ID'),generation=int(os.environ.get('ZUSE_RUNTIME_GENERATION','0')),
        storageIncarnationId=os.environ.get('ZUSE_STORAGE_INCARNATION_ID'),apiUrl=os.environ.get('ZUSE_API_URL'),
        retained=os.environ.get('ZUSE_RUNTIME_RETAINED')=='true')
    if not request['retained']:request['bootTokenFile']=os.environ.get('ZUSE_RUNTIME_BOOT_TOKEN_FILE')
    result=rpc(args.data_dir,request);require(result.get('status')=='accepted','runtime_supervisor_not_accepted')

if __name__=='__main__':
    try:main()
    except (Rejected,OSError,ValueError,subprocess.SubprocessError):raise SystemExit('runtime_supervisor_handoff_failed') from None
