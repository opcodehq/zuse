"""Actual Chromium IndexedDB/WebCrypto persistence acceptance; no account/provider.

Requires installed agent-browser and Chrome. Each run owns temporary profiles and
named sessions. This tests the real browser store, not the wasm authorization UI.
"""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import unittest
import uuid

SOURCE = Path(__file__).resolve().parents[1] / 'src/history_cache.js'


class BrowserCache(unittest.TestCase):
    def test_encrypted_profile_reopen_fresh_device_pages_and_revocation(self):
        browser = shutil.which('agent-browser')
        chrome = shutil.which('google-chrome') or shutil.which('chromium')
        self.assertTrue(browser and chrome, 'Install browser test prerequisites; do not substitute fake IndexedDB')
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == '/history_cache.js':
                    data, kind = SOURCE.read_bytes(), 'text/javascript'
                elif self.path == '/':
                    data, kind = b'<script type="module">window.cache=await import("/history_cache.js");window.ready=true</script>', 'text/html'
                else:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header('Content-Type', kind)
                self.send_header('Cache-Control', 'no-store')
                self.end_headers()
                self.wfile.write(data)
            def log_message(self, *args):
                pass
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        origin = f'http://127.0.0.1:{server.server_port}'
        namespace = json.dumps([origin, 'https://synthetic-api.example.test', 'actor', 'personal'], separators=(',', ':'))
        prefix = 'history-cache-' + uuid.uuid4().hex
        sessions = [prefix + '-retained', prefix + '-fresh']
        def command(session, *args, script=None):
            result = subprocess.run([browser, '--session', session, *args], input=script,
                                    text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
            return result.stdout
        def evaluate(session, body):
            return json.loads(command(session, 'eval', '--stdin', script='(async()=>{const ns=' + json.dumps(namespace) + ';' + body + '})()'))
        with tempfile.TemporaryDirectory(prefix='zuse-encrypted-history-') as directory:
            try:
                def open_profile(index):
                    command(sessions[index], '--profile', str(Path(directory) / str(index)), '--executable-path', chrome, 'open', origin)
                    command(sessions[index], 'wait', '--fn', 'window.ready === true')
                open_profile(0)
                first = evaluate(sessions[0], '''
                    await cache.cacheIdentity('actor/personal');
                    for(const [id,text] of [['head','PRIVATE_HEAD_SENTINEL'],['page:100','PRIVATE_OLDER_PAGE_100'],['page:50','PRIVATE_OLDER_PAGE_50']]){
                        if(!await cache.cachePut(ns,id,'workspace',text))throw Error('store-write-failed');
                    }
                    const db=await new Promise((ok,no)=>{const r=indexedDB.open('zuse-cloud-history-v1',1);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error)});
                    const get=(store,id)=>new Promise((ok,no)=>{const r=db.transaction(store).objectStore(store).get(id);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error)});
                    const key=await get('keys',ns),row=await get('entries',ns+':head');
                    if(key.extractable!==false||key.algorithm.name!=='AES-GCM')throw Error('key-exportable');
                    let denied=false;try{await crypto.subtle.exportKey('raw',key)}catch{denied=true}if(!denied)throw Error('key-export-succeeded');
                    if(!(row.ciphertext instanceof ArrayBuffer)||JSON.stringify(row).includes('PRIVATE_HEAD_SENTINEL')||new TextDecoder().decode(row.ciphertext).includes('PRIVATE_HEAD_SENTINEL'))throw Error('plaintext-record');
                    db.close();return {encrypted:true,nonExtractable:true,records:3};
                ''')
                command(sessions[0], 'close')
                # A new daemon/session makes the browser-process boundary explicit.
                sessions[0] = prefix + '-reopened'
                open_profile(0)
                retained = evaluate(sessions[0], '''
                    for(const [id,text] of [['head','PRIVATE_HEAD_SENTINEL'],['page:100','PRIVATE_OLDER_PAGE_100'],['page:50','PRIVATE_OLDER_PAGE_50']])if(await cache.cacheGet(ns,id)!==text)throw Error('profile-reopen-miss');
                    for(const changed of [[location.origin,'https://other-api.example.test','actor','personal'],[location.origin,'https://synthetic-api.example.test','other','personal'],[location.origin,'https://synthetic-api.example.test','actor','organization:other']])if(await cache.cacheGet(JSON.stringify(changed),'head')!==null)throw Error('scope-leak');
                    return {retainedAcrossBrowserRestart:true,pages:2,scopeIsolation:true};
                ''')
                open_profile(1)
                fresh = evaluate(sessions[1], '''
                    if(await cache.cacheGet(ns,'head')!==null||await cache.cacheGet(ns,'page:100')!==null)throw Error('fresh-device-hit');
                    return {freshDeviceMiss:true};
                ''')
                corrupt = evaluate(sessions[0], '''
                    const db=await new Promise((ok,no)=>{const r=indexedDB.open('zuse-cloud-history-v1',1);r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error)});
                    const row=await new Promise((ok,no)=>{const r=db.transaction('entries').objectStore('entries').get(ns+':page:100');r.onsuccess=()=>ok(r.result);r.onerror=()=>no(r.error)});
                    new Uint8Array(row.ciphertext)[0]^=1;
                    await new Promise((ok,no)=>{const tx=db.transaction('entries','readwrite');tx.objectStore('entries').put(row);tx.oncomplete=ok;tx.onerror=()=>no(tx.error)});db.close();
                    if(await cache.cacheGet(ns,'page:100')!==null||await cache.cacheGet(ns,'page:50')!=='PRIVATE_OLDER_PAGE_50')throw Error('corruption-not-isolated');
                    await cache.cacheIdentity('different-actor/personal');
                    if(await cache.cacheGet(ns,'head')!==null||await cache.cacheGet(ns,'page:50')!==null)throw Error('identity-purge-failed');
                    return {corruptionFailsClosed:true,otherPageRetained:true,identityChangePurged:true};
                ''')
                report = dict(version=1,actualBrowser=True,sourceSha256=hashlib.sha256(SOURCE.read_bytes()).hexdigest(),**first,**retained,**fresh,**corrupt)
                print(json.dumps(report, sort_keys=True))
            finally:
                for session in sessions:
                    subprocess.run([browser, '--session', session, 'close'], capture_output=True, timeout=15)


if __name__ == '__main__':
    unittest.main()
