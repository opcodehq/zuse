#!/usr/bin/env python3
"""Read only the fixed supervisor path and its declared resident's Unix socket FDs."""
import errno,json,os,socket,stat,struct
from pathlib import Path
ADDRESS='/home/zuse/.zuse-data/.runtime-supervision/control.sock'
def diag(inode):
    payload=struct.pack('=BBHIIIII',socket.AF_UNIX,0,0,0xffffffff,inode,0x12,0xffffffff,0xffffffff)
    frame=struct.pack('=IHHII',16+len(payload),20,0x301,771,0)+payload
    found=None
    with socket.socket(socket.AF_NETLINK,socket.SOCK_RAW,4) as probe:
        probe.settimeout(1);probe.bind((0,0));probe.send(frame)
        while found is None:
            packet=probe.recv(65536);offset=0
            while offset+16<=len(packet):
                length,kind,_,seq,_=struct.unpack_from('=IHHII',packet,offset)
                if length<16 or offset+length>len(packet):return {'error':'INVALID_DIAG_RESPONSE'}
                if kind==3:return {'error':'NOT_OBSERVED'}
                if kind==2:
                    code=abs(struct.unpack_from('=i',packet,offset+16)[0]);return {'error':errno.errorcode.get(code,'UNAVAILABLE')}
                if kind==20 and seq==771 and length>=32 and struct.unpack_from('=I',packet,offset+20)[0]==inode:
                    found=packet[offset:offset+length];break
                offset+=(length+3)&~3
    data=found;length=len(data)
    family,typ,state,_,ino,_,_=struct.unpack_from('=BBBBIII',data,16)
    result=dict(inode=ino,type=typ,state=state)
    offset=32
    while offset+4<=length:
        size,kind=struct.unpack_from('=HH',data,offset)
        if size<4 or offset+size>length:break
        body=data[offset+4:offset+size]
        if kind==1 and len(body)==8:result['vfsInode'],result['vfsDevice']=struct.unpack('=II',body)
        if kind==4 and len(body)==8:result['receiveQueue'],result['backlogLimit']=struct.unpack('=II',body)
        if kind==6 and len(body)==1:result['shutdownMask']=body[0]
        offset+=(size+3)&~3
    return result

def observe(address=ADDRESS, resident=None):
    record_error=None
    if resident is None:
        try:
            record=json.loads(Path(address).with_name('supervisor.json').read_text());resident=record['residentPid']
            if not isinstance(resident,int) or resident<1:raise ValueError()
        except (OSError,ValueError,KeyError) as error:record_error=errno.errorcode.get(getattr(error,'errno',None),'RECORD_UNAVAILABLE');resident=None
    result={'version':1,'residentPid':resident}
    if record_error:result['recordError']=record_error
    try:
        m=os.lstat(address);result['path']={'inode':m.st_ino,'device':m.st_dev,'mtimeNs':m.st_mtime_ns,'ctimeNs':m.st_ctime_ns,'socket':stat.S_ISSOCK(m.st_mode)}
    except OSError as error:result['path']={'error':errno.errorcode.get(error.errno,'UNAVAILABLE')}
    if resident is not None and 'inode' in result['path']:
        try:result['residentRootSamePath']=os.stat('/proc/'+str(resident)+'/root'+address).st_ino==result['path']['inode']
        except OSError as error:result['residentRootError']=errno.errorcode.get(error.errno,'UNAVAILABLE')
    result['sockets']=[]
    if resident is None:return result
    try:fds=list(Path('/proc',str(resident),'fd').iterdir())
    except OSError as error:
        result['residentProcessError']=errno.errorcode.get(error.errno,'UNAVAILABLE');return result
    for fd in fds:
        try:
            target=os.readlink(fd)
            if not target.startswith('socket:['):continue
            inode=int(target[8:-1]);fact=diag(inode);fact['fd']=int(fd.name)
            if 'error' in fact and len(result.setdefault('diagnosticUnavailable',[]))<16:result['diagnosticUnavailable'].append({'fd':int(fd.name),'error':fact['error']})
            # Only metadata for the fixed supervised pathname is emitted.
            if fact.get('vfsInode') is not None:
                fact['pathInodeMatchesBoundVfs']=fact['vfsInode']==result['path'].get('inode')
                result['sockets'].append(fact)
        except OSError as error:result['diagnosticError']=errno.errorcode.get(error.errno,'UNAVAILABLE')
    return result
if __name__=='__main__':
    try:result=observe()
    except Exception as error:result={'version':1,'diagnosticFailureClass':type(error).__name__}
    print(json.dumps(result,sort_keys=True))
