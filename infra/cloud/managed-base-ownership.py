#!/usr/bin/env python3
"""Root-only hardening of Boat's proven provider-owned /opt mount directory.

No recursion, content changes, symlink following or task-image acceptance. The
publisher runs this only in a new isolated clean builder, before overlay uploads.
"""
import json,os,stat
from pathlib import Path

def require(ok,code):
 if not ok:raise ValueError(code)
def fact(m):return dict(uid=m.st_uid,gid=m.st_gid,mode=oct(stat.S_IMODE(m.st_mode)),inode=str(m.st_ino),device=str(m.st_dev))
def harden(path=Path('/opt'),*,anchor=Path('/')):
 require(os.geteuid()==0,'managed_ancestor_requires_root')
 path=Path(path);anchor=Path(anchor);require(path!=anchor and anchor in path.parents,'managed_ancestor_path_invalid')
 for parent in path.parents:
  m=parent.lstat();require(stat.S_ISDIR(m.st_mode) and not stat.S_ISLNK(m.st_mode) and m.st_uid==0 and not m.st_mode&0o022,'managed_ancestor_parent_not_protected')
  if parent==anchor:break
 fd=os.open(path,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW|os.O_CLOEXEC)
 try:
  original=os.fstat(fd);require(stat.S_ISDIR(original.st_mode) and original.st_uid in (0,1000) and original.st_gid in (0,1000) and stat.S_IMODE(original.st_mode)==0o755,'managed_ancestor_unexpected_provider_metadata')
  # Freeze the single verified directory before accepting any of its children.
  # A preexisting changed native/helper payload still fails the later SHA checks.
  os.fchown(fd,0,0);os.fchmod(fd,0o755)
  final=os.fstat(fd);visible=path.lstat()
  require(not stat.S_ISLNK(visible.st_mode) and (visible.st_ino,visible.st_dev)==(original.st_ino,original.st_dev)==(final.st_ino,final.st_dev) and final.st_uid==final.st_gid==0 and stat.S_IMODE(final.st_mode)==0o755,'managed_ancestor_hardening_unconfirmed')
  return dict(version=1,path=str(path),before=fact(original),after=fact(final),recursive=False)
 finally:os.close(fd)
if __name__=='__main__':
 try:print(json.dumps(dict(version=2,recursive=False,paths=[harden(Path(path)) for path in ('/opt','/usr/local','/usr/local/bin','/usr/local/lib')]),sort_keys=True))
 except (OSError,ValueError):raise SystemExit('managed_base_ancestor_hardening_rejected') from None
