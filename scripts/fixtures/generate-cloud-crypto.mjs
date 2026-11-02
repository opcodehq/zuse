// Independent WebCrypto vectors for the existing cloud envelope/AAD contracts.
import { webcrypto } from "node:crypto";
import { writeFile } from "node:fs/promises";
const { subtle } = webcrypto;
const b64 = (bytes) => Buffer.from(bytes).toString("base64url");
const utf8 = (value) => new TextEncoder().encode(value);
const raw = Uint8Array.from({ length: 32 }, (_, i) => i);
const iv = Uint8Array.from({ length: 12 }, (_, i) => i);
const key = await subtle.importKey("raw", raw, "AES-GCM", false, ["encrypt"]);
async function envelope(aad, plaintext) {
  const ciphertext = await subtle.encrypt({ name: "AES-GCM", iv, additionalData: utf8(aad) }, key, utf8(plaintext));
  return JSON.stringify({ version: 1, iv: b64(iv), ciphertext: b64(ciphertext) });
}
const context = "message\nuser_fixture\nworkspace_fixture\nmessage_fixture";
const plaintext = "fixture message\nwith a second line";
const transcriptKey = b64(Uint8Array.from({ length: 32 }, (_, i) => i + 32));
const archivePayload = JSON.stringify(["user_fixture", "workspace_fixture", "operation_fixture", 3600000, "write"]);
const hmac = await subtle.importKey("raw", raw, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
const digest = await subtle.sign("HMAC", hmac, utf8(`zuse-cloud-api-digest-v1\nworkspace-archive\n${archivePayload}`));
// This generated private key belongs only to a synthetic interoperability fixture.
const pair = await subtle.generateKey({ name: "RSA-OAEP", modulusLength: 2048, publicExponent: new Uint8Array([1,0,1]), hash: "SHA-256" }, true, ["encrypt", "decrypt"]);
const jwk = await subtle.exportKey("jwk", pair.publicKey);
const thumbprint = b64(await subtle.digest("SHA-256", utf8(JSON.stringify({ e: jwk.e, kty: "RSA", n: jwk.n }))));
const fixture = {
  source: "WebCrypto; original Zuse AES-GCM envelope and AAD contracts at 2016cf4",
  masterKey: b64(raw), context, plaintext,
  apiEnvelope: await envelope(`zuse-cloud-api-v1\n${context}`, plaintext),
  transcriptKey,
  transcriptEnvelope: await envelope("zuse-cloud-transcript-wrapping-v1\nuser_fixture\nworkspace_fixture", transcriptKey),
  archivePayload, archiveDigest: Buffer.from(digest).toString("hex"),
  runtimePublicJwk: jwk,
  runtimePrivatePkcs8: b64(await subtle.exportKey("pkcs8", pair.privateKey)),
  runtimeThumbprint: thumbprint,
};
await writeFile("crates/cloud/tests/fixtures/cloud-crypto.json", `${JSON.stringify(fixture, null, 2)}\n`);
