"""Verified fixed-path loader; argv contains only helper SHA and base64 metadata.

Refusals print one JSON line with a fixed `fresh_boot_loader_*` code. On a
fresh Boat restore /usr/local and /usr/local/lib briefly show the provider's
uid 1000 before root ownership appears (observed for ~10 s after create), so
the control plane treats `fresh_boot_loader_path_unsafe` as not yet ready; the
helper itself never runs while any ancestor is unsafe."""
import hashlib,json,os,stat,sys
from pathlib import Path
path=Path('/usr/local/lib/zuse/managed-base-fresh-boot.py')
def reject(code):
 print(json.dumps({'version':1,'status':'rejected','code':'fresh_boot_loader_'+code}));raise SystemExit(1)
try:
 for ancestor in (path,*path.parents):
  m=ancestor.lstat()
  if m.st_uid!=0 or stat.S_ISLNK(m.st_mode) or m.st_mode&0o022:reject('path_unsafe')
 fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
except OSError:reject('path_unreadable')
try:
 m=os.fstat(fd)
 if not stat.S_ISREG(m.st_mode) or m.st_nlink!=1 or m.st_size>131072:reject('file_invalid')
 # read() may return fewer bytes than asked; read to end of file, bounded.
 body=b''
 while len(body)<=131072:
  chunk=os.read(fd,131073-len(body))
  if not chunk:break
  body+=chunk
 after=os.fstat(fd)
 if (m.st_dev,m.st_ino,m.st_size,m.st_mtime_ns,m.st_ctime_ns)!=(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns,after.st_ctime_ns):reject('file_changed')
 if len(sys.argv)!=3 or len(sys.argv[2])>32768:reject('arguments_invalid')
 if hashlib.sha256(body).hexdigest()!=sys.argv[1]:reject('helper_digest_mismatch')
except OSError:reject('read_failed')
finally:os.close(fd)
sys.argv=[str(path),'--request-base64',sys.argv[2]]
exec(compile(body,str(path),'exec'),{'__name__':'__main__','__file__':str(path)})
