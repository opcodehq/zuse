// Private browser cache: ciphertext and non-extractable WebCrypto keys live in
// IndexedDB. Authorization is enforced by the Rust transport before each read.
const DB = 'zuse-cloud-history-v1';
const LIMIT = 32 * 1024 * 1024;
const RECORD_LIMIT = 1024 * 1024;
const MAX_RECORDS = 256;
const AGE = 7 * 24 * 60 * 60 * 1000;
const epochs = new Map();
let globalEpoch = 0;
let channel;
try { channel = typeof BroadcastChannel === 'function' ? new BroadcastChannel(DB) : null; } catch (_) {}
let database;
function open() {
  if (!database) database = new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 1);
    request.onupgradeneeded = () => {
      request.result.createObjectStore('keys');
      const entries = request.result.createObjectStore('entries', {keyPath: 'id'});
      entries.createIndex('namespace', 'namespace');
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => { database = undefined; reject(request.error); };
    request.onblocked = () => reject(new Error('cache unavailable'));
  });
  return database;
}
function request(request) {
  return new Promise((resolve,reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
function completed(transaction) {
  return new Promise((resolve,reject) => {
    transaction.oncomplete = resolve;
    transaction.onabort = transaction.onerror = () => reject(transaction.error);
  });
}
async function keyFor(db, namespace, create) {
  let key = await request(db.transaction('keys').objectStore('keys').get(namespace));
  if (!key && create) {
    const candidate = await crypto.subtle.generateKey({name:'AES-GCM',length:256},false,['encrypt','decrypt']);
    try {
      const tx = db.transaction('keys','readwrite');
      tx.objectStore('keys').add(candidate,namespace);
      await completed(tx);
      key = candidate;
    } catch (_) { key = await request(db.transaction('keys').objectStore('keys').get(namespace)); }
  }
  return key;
}
async function evict(db, namespace) {
  const records = await request(db.transaction('entries').objectStore('entries').index('namespace').getAll(namespace));
  records.sort((a,b) => b.usedAt - a.usedAt);
  let bytes = 0;
  const remove = [];
  for (let i=0;i<records.length;i++) {
    const entry=records[i]; bytes += entry.bytes;
    if (i>=MAX_RECORDS || bytes>LIMIT || Date.now()-entry.usedAt>AGE) remove.push(entry.id);
  }
  if (remove.length) {
    const tx=db.transaction('entries','readwrite');
    for (const id of remove) tx.objectStore('entries').delete(id);
    await completed(tx);
  }
}
export async function cachePut(namespace, identity, workspace, text) {
  try {
    const epoch=epochs.get(namespace)||0;
    const all=globalEpoch;
    const bytes=new TextEncoder().encode(text);
    if (bytes.length>RECORD_LIMIT) return false;
    const db=await open(); const key=await keyFor(db,namespace,true);
    const iv=crypto.getRandomValues(new Uint8Array(12));
    const id=namespace+':'+identity;
    const ciphertext=await crypto.subtle.encrypt({name:'AES-GCM',iv,additionalData:new TextEncoder().encode(id)},key,bytes);
    if ((epochs.get(namespace)||0)!==epoch || globalEpoch!==all) { await cacheClear(namespace,false); return false; }
    const tx=db.transaction('entries','readwrite');
    tx.objectStore('entries').put({id,namespace,workspace,iv,ciphertext,bytes:ciphertext.byteLength,usedAt:Date.now()});
    await completed(tx); await evict(db,namespace); return true;
  } catch (_) { return false; }
}
export async function cacheGet(namespace, identity) {
  try {
    const epoch=epochs.get(namespace)||0;
    const all=globalEpoch;
    const db=await open(); const id=namespace+':'+identity;
    const entry=await request(db.transaction('entries').objectStore('entries').get(id));
    if (!entry || Date.now()-entry.usedAt>AGE) return null;
    const key=await keyFor(db,namespace,false); if (!key) return null;
    const plain=await crypto.subtle.decrypt({name:'AES-GCM',iv:entry.iv,additionalData:new TextEncoder().encode(id)},key,entry.ciphertext);
    if ((epochs.get(namespace)||0)!==epoch || globalEpoch!==all) return null;
    return new TextDecoder('utf-8',{fatal:true}).decode(plain);
  } catch (_) { return null; }
}
export async function cacheClear(namespace, broadcast=true) {
  epochs.set(namespace,(epochs.get(namespace)||0)+1);
  if (broadcast && channel) channel.postMessage(namespace);
  try {
    const db=await open();
    const records=await request(db.transaction('entries').objectStore('entries').index('namespace').getAllKeys(namespace));
    const tx=db.transaction(['entries','keys'],'readwrite');
    for (const id of records) tx.objectStore('entries').delete(id);
    tx.objectStore('keys').delete(namespace);
    await completed(tx);
  } catch (_) {}
}
export async function cacheClearAll() {
  globalEpoch++;
  try {
    const db=await open();
    const namespaces=await request(db.transaction('keys').objectStore('keys').getAllKeys());
    await Promise.all(namespaces.filter(n=>typeof n==='string').map(n=>cacheClear(n)));
  } catch (_) {}
}
export async function cacheIdentity(identity) {
  try {
    const db=await open();
    const current=await request(db.transaction('keys').objectStore('keys').get('__identity__'));
    if (current!==identity) {
      await cacheClearAll();
      const tx=db.transaction('keys','readwrite');tx.objectStore('keys').put(identity,'__identity__');await completed(tx);
    }
  } catch (_) {}
}
if (channel) channel.onmessage = event => {
  if (typeof event.data==='string') cacheClear(event.data,false);
};
