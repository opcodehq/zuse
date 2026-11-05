"""Verified fixed-path loader; argv contains only helper SHA and base64 metadata."""
import hashlib,os,stat,sys
from pathlib import Path
path=Path('/usr/local/lib/zuse/managed-base-fresh-boot.py')
for ancestor in (path,*path.parents):
 m=ancestor.lstat()
 assert m.st_uid==0 and not stat.S_ISLNK(m.st_mode) and not m.st_mode&0o022
fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
try:
 m=os.fstat(fd);assert stat.S_ISREG(m.st_mode) and m.st_nlink==1 and m.st_size<=131072
 body=os.read(fd,131073);after=os.fstat(fd)
 assert (m.st_dev,m.st_ino,m.st_size,m.st_mtime_ns,m.st_ctime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns,after.st_ctime_ns)
 assert len(sys.argv)==3 and len(sys.argv[2])<=32768 and hashlib.sha256(body).hexdigest()==sys.argv[1]
finally:os.close(fd)
sys.argv=[str(path),'--request-base64',sys.argv[2]]
exec(compile(body,str(path),'exec'),{'__name__':'__main__','__file__':str(path)})
