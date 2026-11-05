#!/usr/bin/env python3
"""Private stdin compatibility probe. Counts only, no key/account/plaintext output."""
import base64
import json
import sys

def proof(value):
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    def decode(text):
        if not isinstance(text,str):raise ValueError()
        return base64.urlsafe_b64decode(text+'='*((-len(text))%4))
    key=decode(value['key']);rows=value['rows']
    if len(key)!=32 or not isinstance(rows,list) or len(rows)>64:raise ValueError()
    cipher=AESGCM(key);opened=failed=0
    for row in rows:
        plaintext=None
        try:
            account=row['accountId'];sealed=row['sealed']
            if not isinstance(account,str) or len(account)>200 or not isinstance(sealed,str) or len(sealed)>131072:raise ValueError()
            envelope=json.loads(sealed)
            if envelope['version']!=1:raise ValueError()
            iv=decode(envelope['iv']);body=decode(envelope['ciphertext'])
            if len(iv)!=12 or len(body)<16:raise ValueError()
            plaintext=bytearray(cipher.decrypt(iv,body,('zuse-cloud-api-v1\ngithub-user\n'+account).encode()))
            opened+=1
        except Exception:failed+=1
        finally:
            if plaintext is not None:plaintext[:]=b'\0'*len(plaintext)
    return dict(metadataOnly=True,checked=len(rows),opened=opened,failed=failed,keyMatchesAllSamples=opened>0 and failed==0)

if __name__=='__main__':
    try:
        value=json.loads(sys.stdin.buffer.read(16*1024*1024+1))
        print(json.dumps(proof(value)))
    except Exception:
        print(json.dumps(dict(metadataOnly=True,checked=0,opened=0,failed=0,keyMatchesAllSamples=False,code='private_crypto_proof_unavailable')))
