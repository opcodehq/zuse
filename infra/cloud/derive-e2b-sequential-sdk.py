#!/usr/bin/env python3
"""Derive the pinned SDK with one-at-a-time archive/PUT/cleanup; no network."""
from pathlib import Path
import argparse,hashlib,json,os
UPSTREAM_SHA256='1c96b58fb06dd23f45d869a20800356b8712e33ff6c7b3bd04034e2fac112eaa'
DERIVATION='zuse-sequential-upload-v1'
def derive(raw):
 if hashlib.sha256(raw).hexdigest()!=UPSTREAM_SHA256:raise ValueError('e2b_sdk_upstream_changed')
 source=raw.decode();start='\t\tconst uploadPromises = instructionsWithHashes.map(async (instruction, index) => {';end='\t\t});\n\t\tawait Promise.all(uploadPromises);'
 if source.count(start)!=1 or source.count(end)!=1:raise ValueError('e2b_sdk_upload_seam_changed')
 source=source.replace(start,'\t\tfor (const [index, instruction] of instructionsWithHashes.entries()) {',1)
 pos=source.index('\t\tfor (const [index, instruction]');stop=source.index(end,pos);block=source[pos:stop]
 if block.count('if (instruction.type !== "COPY") return;')!=1:raise ValueError('e2b_sdk_upload_return_changed')
 source=source[:pos]+block.replace('if (instruction.type !== "COPY") return;','if (instruction.type !== "COPY") continue;',1)+source[stop:]
 source=source.replace(end,'\t\t}',1)
 return ('// Explicit local derivation: upstream e2b 2.62.0, '+DERIVATION+'.\n'+source).encode()
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--upstream',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();value=derive(a.upstream.read_bytes());fd=os.open(a.output,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o444)
 with os.fdopen(fd,'wb')as f:f.write(value);f.flush();os.fsync(f.fileno())
 print(json.dumps({'derivation':DERIVATION,'upstreamSha256':UPSTREAM_SHA256,'derivedSha256':hashlib.sha256(value).hexdigest(),'bytes':len(value),'providerEffects':0}))
if __name__=='__main__':main()
