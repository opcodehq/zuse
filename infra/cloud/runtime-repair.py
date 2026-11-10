#!/usr/bin/env python3
"""Fenced retirement and native v1 crash reconciliation; no prompt replay."""
import ctypes
import errno
import json
import os
from pathlib import Path
import signal
import sqlite3
import stat
import subprocess
import time
import urllib.parse

MAX_RESUME_ATTEMPTS=4294967295

def enable_subreaper():
    return ctypes.CDLL(None).prctl(36,1,0,0,0)==0

def process(pid):
    try:
        raw=Path('/proc',str(pid),'stat').read_text();fields=raw[raw.rfind(')')+2:].split()
        return dict(pid=pid,parent=int(fields[1]),start=fields[19],zombie=fields[0]=='Z')
    except (OSError,ValueError,IndexError):return None

def owned_processes(supervisor):
    processes={}
    for entry in Path('/proc').iterdir():
        if entry.name.isdigit():
            info=process(int(entry.name))
            if info:processes[info['pid']]=info
    owned={supervisor};changed=True
    while changed:
        changed=False
        for pid,info in processes.items():
            if pid not in owned and info['parent'] in owned:owned.add(pid);changed=True
    # Systemd retains descendants in its cgroup even when they reparent/setsid.
    try:
        for line in Path('/proc/self/cgroup').read_text().splitlines():
            _,_,name=line.split(':',2)
            if 'zuse-runtime-supervisor.service' in Path(name).parts:
                for group in [Path('/sys/fs/cgroup')/name.lstrip('/')]:
                    for membership in [group/'cgroup.procs',*group.glob('**/cgroup.procs')]:
                        for value in membership.read_text().split():
                            pid=int(value)
                            if pid in processes:owned.add(pid)
    except (OSError,ValueError):pass
    return [processes[p] for p in owned if p!=supervisor and p in processes and not processes[p]['zombie']]

def signal_fenced(info,sig):
    current=process(info['pid'])
    if not current or current['start']!=info['start'] or current['zombie']:return
    try:os.kill(info['pid'],sig)
    except ProcessLookupError:return
    except PermissionError:
        # Root tools inherit the runtime's service/ancestry. Recheck PID birth in
        # the privileged helper, never issue a kill against an unfenced number.
        code="import os,sys,signal;from pathlib import Path;p=Path('/proc',sys.argv[1],'stat');s=p.read_text();f=s[s.rfind(')')+2:].split();os.kill(int(sys.argv[1]),int(sys.argv[3])) if f[19]==sys.argv[2] else None"
        result=subprocess.run(['/usr/bin/sudo','-n','/usr/bin/python3','-c',code,str(info['pid']),info['start'],str(sig)],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=3,env={'PATH':'/usr/bin:/bin','HOME':'/','LANG':'C.UTF-8'})
        remaining=process(info['pid'])
        if result.returncode and remaining and not remaining['zombie']:raise RuntimeError('repair_child_retirement_denied')

def retire_owned(supervisor,timeout=8):
    """Called before a new child exists. Stop forks, kill, reap; confirm absence."""
    if supervisor!=os.getpid():raise RuntimeError('repair_supervisor_pid_invalid')
    deadline=time.monotonic()+timeout;retired=set()
    while time.monotonic()<deadline:
        children=owned_processes(supervisor)
        if not children:
            # A second observation catches a fork/reparent concurrent with scan.
            time.sleep(.02)
            if not owned_processes(supervisor):return len(retired)
            continue
        for child in children:signal_fenced(child,signal.SIGSTOP)
        for child in children:signal_fenced(child,signal.SIGKILL);retired.add((child['pid'],child['start']))
        while True:
            try:
                pid,_=os.waitpid(-1,os.WNOHANG)
                if not pid:break
            except ChildProcessError:break
        time.sleep(.02)
    raise RuntimeError('repair_orphan_processes_uncertain')

def private_json(path):
    fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
    try:
        info=os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid() or info.st_mode&0o077 or info.st_size>32768:raise RuntimeError('repair_storage_metadata_invalid')
        with os.fdopen(fd,'rb',closefd=False) as stream:return json.load(stream)
    finally:os.close(fd)

def reconcile(data,identity,commands):
    """Caller holds the exclusive native runtime lock and has retired all children."""
    marker=private_json(data/'rust-runtime-storage.json')
    if marker.get('version')!=1 or marker.get('workspaceId')!=identity['workspaceId'] or marker.get('storage',{}).get('storageIncarnationId')!=identity['storageIncarnationId'] or marker.get('storage',{}).get('format')!='zuse-rust-engine-v1' or not marker.get('ready'):raise RuntimeError('repair_storage_fenced')
    relative=Path(marker['database'])
    if relative.is_absolute() or not relative.parts or any(p in ('..','.') for p in relative.parts):raise RuntimeError('repair_storage_path_invalid')
    database=data/relative
    for path in [database,*database.parents]:
        if path==data.parent:break
        if path.is_symlink():raise RuntimeError('repair_storage_symlink')
    journals=database.parent/'journals'
    files=list(journals.glob('*.jsonl'))
    if not journals.is_dir() or len(files)>2048:raise RuntimeError('repair_journal_scope_invalid')
    interrupted=[]
    for path in files:
        info=path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid():raise RuntimeError('repair_journal_invalid')
        last=None
        with path.open('rb') as stream:
            size=stream.seek(0,2);start=max(0,size-1048576);stream.seek(start);tail=stream.read(1048576)
        lines=tail.splitlines()
        if start and lines:lines=lines[1:]
        for line in reversed(lines):
            try:last=json.loads(line);break
            except (ValueError,UnicodeDecodeError):continue
        if size and (last is None or last.get('event',{}).get('type')!='done'):
            # Existing engine recovery closes abandoned streams, but its revival
            # ledger must be exhausted first to prevent bare-prompt redispatch.
            target=path.with_suffix('.resume');temporary=target.with_name('.runtime-write-repair-'+os.urandom(8).hex())
            fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
            with os.fdopen(fd,'w') as out:out.write(str(MAX_RESUME_ATTEMPTS));out.flush();os.fsync(out.fileno())
            os.replace(temporary,target);interrupted.append(path.stem)
    if interrupted:
        directory=os.open(journals,os.O_DIRECTORY|os.O_CLOEXEC|os.O_NOFOLLOW)
        try:os.fsync(directory)
        finally:os.close(directory)
    db=sqlite3.connect('file:'+urllib.parse.quote(str(database))+'?mode=rw',uri=True,timeout=3)
    try:
        db.execute('PRAGMA synchronous=FULL')
        if db.execute('PRAGMA quick_check').fetchone()[0]!='ok':raise RuntimeError('repair_wal_integrity_failed')
        row=db.execute('SELECT workspace_id,account_id,storage_incarnation_id FROM zuse_cloud_storage_identity WHERE singleton=1').fetchone()
        if row!=(identity['workspaceId'],marker['accountId'],identity['storageIncarnationId']):raise RuntimeError('repair_storage_fenced')
        db.execute('BEGIN IMMEDIATE')
        scope=(identity['workspaceId'],identity['storageIncarnationId'])
        # Legacy gateway/native CQ execution does not instantiate Mailbox.
        # Absence is not proof that any requested server command was applied;
        # keep the real engine ledger/history and report those outcomes unknown.
        receipt_schema=db.execute("SELECT type FROM sqlite_master WHERE name='zuse_cloud_command_receipts'").fetchone()
        if receipt_schema is not None and receipt_schema[0]!='table':raise RuntimeError('repair_receipt_schema_invalid')
        count=0;receipts={}
        if receipt_schema is not None:
            count=db.execute('SELECT count(*) FROM zuse_cloud_command_receipts WHERE workspace_id=? AND storage_incarnation_id=?',scope).fetchone()[0]
            now=int(time.time()*1000)
            db.execute('INSERT OR IGNORE INTO processed_commands(command_id,processed_at) SELECT command_id,? FROM zuse_cloud_command_receipts WHERE workspace_id=? AND storage_incarnation_id=?',(now,*scope))
            # Do not let Mailbox::recover infer Applied from the quarantine ledger.
            # Preserve the original lease binding and all existing terminal ACK bytes.
            for cid,fingerprint,raw_lease in db.execute('SELECT command_id,fingerprint,lease_json FROM zuse_cloud_command_receipts WHERE workspace_id=? AND storage_incarnation_id=? AND acknowledgment IS NULL',scope).fetchall():
                lease=json.loads(raw_lease);command=lease.get('command',{})
                if command.get('commandId')!=cid or command.get('workspaceId')!=identity['workspaceId'] or command.get('fingerprint')!=fingerprint or lease.get('storageIncarnationId')!=identity['storageIncarnationId'] or not isinstance(lease.get('leaseToken'),str) or not lease['leaseToken']:raise RuntimeError('repair_lease_binding_invalid')
                ack=dict(commandId=cid,leaseToken=lease['leaseToken'],fingerprint=fingerprint,state='outcome-unknown',category='runtime-repair-interrupted')
                db.execute('UPDATE zuse_cloud_command_receipts SET acknowledgment=?,delivered=0 WHERE command_id=? AND acknowledgment IS NULL',(json.dumps(ack,separators=(',',':')),cid))
            receipts={}
            for command in commands:
                row=db.execute('SELECT fingerprint,acknowledgment FROM zuse_cloud_command_receipts WHERE command_id=? AND workspace_id=? AND storage_incarnation_id=?',(command,*scope)).fetchone()
                if row is None:continue
                fingerprint,ack=row
                bound_ack=json.loads(ack) if ack else {}
                state=bound_ack.get('state') if bound_ack.get('commandId')==command and bound_ack.get('fingerprint')==fingerprint else None
                if state not in ('applied','rejected','expired','cancelled','outcome-unknown'):state=None
                receipts[command]=dict(commandId=command,fingerprint=fingerprint if isinstance(fingerprint,str) and len(fingerprint)<=128 else None,state=state)
        db.commit()
        return dict(interruptedChats=interrupted[:64],interruptedChatCount=len(interrupted),interruptedChatsTruncated=len(interrupted)>64,quarantinedCommandCount=count,receipts=[receipts.get(c,dict(commandId=c,fingerprint=None,state=None)) for c in commands])
    finally:db.close()
