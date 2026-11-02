// Independent WebCrypto wire proof using the original ordered v3 AAD contract.
import { webcrypto } from 'node:crypto';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const { encodedKey, envelope, plaintext, mode } = JSON.parse(input);
const bytes = value => Buffer.from(value, 'base64url');
const aad = e => Buffer.from(JSON.stringify(['zuse-cloud-command-v3', e.workspaceId,
  e.sessionId, e.commandId, e.kind, e.fingerprint, e.schemaVersion, e.keyVersion,
  e.destructionFence, e.createdAt, e.dependencies]));
const aes = await webcrypto.subtle.importKey('raw', bytes(encodedKey), 'AES-GCM', false, ['encrypt','decrypt']);
if (mode === 'decrypt') {
  const value = await webcrypto.subtle.decrypt({name:'AES-GCM',iv:bytes(envelope.iv),additionalData:aad(envelope)}, aes, bytes(envelope.ciphertext));
  process.stdout.write(JSON.stringify({plaintext:Buffer.from(value).toString()}));
} else {
  const mac = await webcrypto.subtle.importKey('raw', bytes(encodedKey), {name:'HMAC',hash:'SHA-256'}, false, ['sign']);
  envelope.fingerprint = 'hmac-sha256:' + Buffer.from(await webcrypto.subtle.sign('HMAC',mac,Buffer.from(plaintext))).toString('base64url');
  const iv = webcrypto.getRandomValues(new Uint8Array(12));
  const ciphertext = await webcrypto.subtle.encrypt({name:'AES-GCM',iv,additionalData:aad(envelope)},aes,Buffer.from(plaintext));
  envelope.iv = Buffer.from(iv).toString('base64url');
  envelope.ciphertext = Buffer.from(ciphertext).toString('base64url');
  process.stdout.write(JSON.stringify({envelope,plaintext}));
}
