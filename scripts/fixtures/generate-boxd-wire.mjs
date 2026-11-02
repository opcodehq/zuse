// Regenerate independent wire fixtures from the official SDK, never the Rust codec.
// Usage: node scripts/fixtures/generate-boxd-wire.mjs /path/to/sdk/package
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { writeFile } from "node:fs/promises";

if (!process.argv[2]) throw new Error("Pass the extracted @boxd-sh/sdk 0.2.14 package directory");
const sdk = await import(pathToFileURL(resolve(process.argv[2], "dist/_generated/definitions.js")));
const hex = (type, value) => Buffer.from(type.encode(type.fromPartial(value)).finish()).toString("hex");
const fixtures = {
  source: "@boxd-sh/sdk@0.2.14",
  createRequest: hex(sdk.CreateVmFromSnapshotRequest, {
    snapshot: "zuse-runtime-fixture", name: "w-fixture", org: "fixture-org", isolated: true,
    config: { srf: { autoSuspendTimeoutSecs: 0, autoDestroyTimeoutSecs: 0 } },
  }),
  createResponse: hex(sdk.CreateVmResponse, { vmId: "vm-fixture", name: "w-fixture", status: "running" }),
  machineRequest: hex(sdk.GetVmRequest, { vmId: "vm-fixture" }),
  missingRequest: hex(sdk.GetVmRequest, { vmId: "missing" }),
  machineResponse: hex(sdk.GetVmResponse, {
    vmId: "vm-fixture", name: "w-fixture", status: "running", vcpu: 4, memoryBytes: 17179869184,
  }),
  resumeResponse: hex(sdk.ResumeVmResponse, { resumeUs: 123456 }),
  idleRequest: hex(sdk.SetAutoHibernateTimeoutRequest, { vmId: "vm-fixture", timeoutSecs: 600 }),
  emptyResponse: hex(sdk.HibernateVmResponse, {}),
  execRequest: hex(sdk.ExecChunk, { vmId: "vm-fixture", command: "printf cloud-ready" }),
  execOutput: hex(sdk.ExecChunk, { data: new TextEncoder().encode("cloud-ready") }),
  execError: hex(sdk.ExecChunk, { data: new TextEncoder().encode("diagnostic"), isStderr: true }),
  execExit: hex(sdk.ExecChunk, { exitCode: 2 }),
  uploadRequest: hex(sdk.UploadFileRequest, {
    vmId: "vm-fixture", path: "/run/zuse-secrets/bootstrap", data: new TextEncoder().encode("fixture-config"),
  }),
  uploadResponse: hex(sdk.UploadFileResponse, { bytesWritten: 14 }),
};
await writeFile("crates/cloud/tests/fixtures/boxd-wire.json", `${JSON.stringify(fixtures, null, 2)}\n`);
