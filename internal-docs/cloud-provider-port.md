# Direct provider port: Boxd and E2B

Research only, October 22, 2026. No provider API, credentials, implementation or tests were used.
Upstream adapter baseline: `swarajbachu/zuse@2016cf4f0cc240e6797916dd5cab1d8bebab89ae`.
SDK reference: official `@boxd-sh/sdk@0.2.14`, downloaded exclusively under `.context/boxd-sdk-reference`.

## Recommendation

Boxd is feasible directly in Rust: use existing HTTP client plus `prost` messages and a small binary grpc-web frame codec. Provider operations do not require Node or a JavaScript sidecar.
The npm artifact includes generated protobuf codecs and exact service definitions, so an unpublished `.proto` is not a wire-contract blocker.
No official Rust SDK or public raw protobuf repository was established. The [official FAQ](https://boxd.sh/faq/) advertises Python and TypeScript SDKs over gRPC; do not assume a similarly named Rust crate is official.
Native gRPC is also established by the official Node transport and can use tonic once messages are represented; grpc-web follows the existing upstream Worker path most closely.
E2B has a simpler JSON lifecycle and Connect-JSON process API with no generated protobuf requirement. It is the fallback for the earliest real workspace if Boxd codec work delays delivery, provided an actual compatible E2B runtime template and key are available.
Neither provider lifecycle adapter by itself provides chat/session compatibility: bootstrap the chosen runtime and connect its supported guest protocol separately.

## Boxd wire and authentication

Primary sources are the published artifact [npm metadata](https://registry.npmjs.org/@boxd-sh/sdk/0.2.14) and [SDK tarball](https://registry.npmjs.org/@boxd-sh/sdk/-/sdk-0.2.14.tgz).
Local files below are under `.context/boxd-sdk-reference/package/dist/`.

- API base: `https://boxd.sh:9443` (`client.js`, `utils.js`). Preserve configurable endpoint resolution for nonproduction clusters.
- Exchange an API key with `POST https://app.boxd.sh/api/v1/auth/token`, JSON `{"api_key":"<secret>"}` and `content-type: application/json`.
- Exchange response requires string `token` and numeric Unix-seconds `expires_at`; coalesce refresh and re-exchange five minutes before expiration (`auth.js`).
- Explicit bearer token mode bypasses API-key exchange. Do not send an API key directly as the RPC bearer.
- Token exchange 401 is authentication failure; 429 retries once using bounded `Retry-After` (SDK cap 60 seconds), then fails visibly.
- RPC URL: `POST <api-base>/boxd.api.v1.BoxdApi/<ExactMethodName>`.
- RPC headers: `content-type: application/grpc-web+proto`, `x-grpc-web: 1`, `authorization: Bearer <token>`.
- Official SDK additionally sends `x-boxd-ts-sdk-version: 0.2.14`; a Rust client should not claim to be the TypeScript SDK without a provider requirement.
- Each request/response DATA frame is flag byte `0x00`, four-byte unsigned big-endian payload length, then binary protobuf.
- Response TRAILER frame has bit `0x80` set; payload is UTF-8 `key: value\r\n`, including `grpc-status` and optional percent-encoded `grpc-message`.
- Parse both trailers-only initial HTTP headers and body trailer frames. HTTP 200 does not prove RPC success. Decode fragmented frames incrementally and enforce explicit size limits.
- Standard protobuf wire types apply; default scalar values can be omitted, unknown fields skipped, uint64/int64 retained as integers rather than JSON floating point.
- The SDK's unary default timeout is sixty seconds. Deadline-exceeded is not retried because a mutation may already have happened.
- Do not blindly replay create/exec on network ambiguity; reconcile deterministic machine identity and durable application operation state.

Sources: `_transportWeb.js`, `_transport.js`, `_retry.js`, `errors.js`, `auth.js`, `client.js`, `utils.js`, `_generated/definitions.js`.

## Boxd exact method/message subset

Notation below is `field-number:type fieldName`; omitted fields are not an instruction to change the provider schema. These are extracted from SDK 0.2.14 generated codecs.

| Operation | Exact RPC | Request and response |
| --- | --- | --- |
| Inspect | `GetVm` | Request `1:string vmId`; response `GetVmResponse` below |
| Create from image | `CreateVmFromSnapshot` | `1:string snapshot, 2:string name, 3:VmConfig config, 4:string org, 5:bool isolated`; returns `CreateVmResponse` |
| Start stopped VM | `StartVm` | `1:string vmId`; response has no fields needed by adapter |
| Resume RAM suspension | `ResumeVm` | `1:string vmId`; response `1:uint64 resumeUs` |
| Hibernate | `HibernateVm` | `1:string vmId`; response has no fields needed by adapter |
| Wake hibernation | `WakeVm` | `1:string vmId`; response has no fields needed by adapter |
| Delete | `DestroyVm` | `1:string vmId`; response has no fields needed by adapter; GetVm returns NOT_FOUND after destruction |
| Exec | `Exec` | Bidirectional `ExecChunk`; grpc-web SDK sends all request frames, ends request, then reads output |
| Small upload | `UploadFile` | `1:string vmId, 2:string path, 3:bytes data`; response `1:uint64 bytesWritten` |
| Chunked upload | `UploadFileStream` | Request stream `UploadFileChunk`; one `UploadFileResponse` |
| Usage | `GetVmUsage` | `1:string vmId, 2:int64 since, 3:int64 until, 4:string org`; seconds, not milliseconds; response below |
| Resize | `ResizeVm` | `1:string vmId, 2:uint32 vcpu, 3:uint64 memoryBytes`; running machine reboots |
| Idle policy | `SetAutoHibernateTimeout` | `1:string vmId, 2:uint32 timeoutSecs` |
| Egress | `SetVmEgressAllow` | `1:string vm, 2:repeated string entries` |
| Snapshot build | `CreateSnapshot` | `1:string vm, 2:string name`; then poll `GetSnapshot` |
| Snapshot inspect | `GetSnapshot` | `1:string name, 2:string org`; response `1:SnapshotInfo snapshot` |
| Machine fork | `ForkVm` | `1:string sourceVmId, 2:string name, 3:VmConfig config, 4:bool shared, 5:repeated string networks, 6:bool isolated`; response `CreateVmResponse` |

- `VmConfig`: `1:uint32 vcpu, 2:uint64 memoryBytes, 3:uint64 diskBytes, 4:SrfConfig srf, 5:NetworkConfig network, 6:repeated VolumeMount volumes`.
- `SrfConfig`: optional `1:uint32 autoSuspendTimeoutSecs`, `2:uint32 autoDestroyTimeoutSecs`. Omitted optional suspend timeout differs from explicit zero (disabled).
- `NetworkConfig`: `1:bool ssh, 2:repeated ProxyEntry proxies`; retain generated nested definitions when exposing these capabilities.
- `CreateVmResponse`: `1:string vmId, 2:string name, 3:string publicIp, 4:string url, 5:string image, 6:string status, 7:uint64 bootTimeMs`.
- `GetVmResponse` identity/state fields: `1:string vmId, 2:string name, 3:string imageRef, 4:string publicIp, 5:string status, 6:string restartPolicy`.
- Its resource/access fields: `7:uint64 diskBytes, 8:uint32 autoSuspendTimeoutSecs, 9:uint32 sshPort, 10:string org, 11:string billingOrg, 12:string accessDomain, 13:uint32 autoHibernateTimeoutSecs, 14:string source, 15:uint32 vcpu, 16:uint64 memoryBytes`.
- Its timestamps/scope: `17:int64 createdAt, 18:int64 hibernatedAtMs, 19:uint32 autoDestroyTimeoutSecs, 20:int64 bootTimeMs, 21:int64 lastConnectedMs, 22:string orgId, 23:string billingOrgId, 24:VmSource sourceInfo, 26:bool isolated, 29:bool botProtection`; other fields remain in the generated source.
- `ExecChunk`: `1:bytes data, 2:bool stdin, 3:bool tty, 4:string command, 5:string vmId, 6:int32 exitCode, 7:uint32 cols, 8:uint32 rows, 9:bool windowChange, 10:bool isStderr, 11:string deviceId`.
- Exec initial frame sets vmId/command/tty/cols/rows; command is a shell line. SDK shell-quotes array arguments, prefixes env assignments and implements cwd with `cd <quoted> && ...`.
- Split response bytes into stdout/stderr using isStderr; retain reported exitCode and require successful stream termination.
- Provider grpc-web exec is suitable for bounded provisioning commands. It is half-duplex; use the runtime's own retained PTY/gateway for the app terminal. Canceling provider Exec kills its remote command group, so start durable runtime processes through the established detached supervisor.
- `UploadFileChunk`: `1:string vmId, 2:string path, 3:uint64 totalSize, 4:bytes data`; first chunk includes identity/path/total size; later chunks include data only.
- SDK uses unary upload at up to 3 MiB, otherwise 1 MiB data chunks. Chunked grpc-web request contains concatenated frames; response bytesWritten confirms transfer size.
- `GetVmUsageResponse`: `1:int64 periodStart, 2:int64 periodEnd, 3:string currency, 4:UsageRates rates, 5:VmUsage usage`.
- `UsageRates`: `1:int64 vcpuHourMicro, 2:int64 ramGibHourMicro, 3:int64 diskGibHourMicro`.
- `VmUsage`: `1:string vmId, 2:string name, 3:string owner, 4:string ownerName, 5:string sharing, 6:string status, 7:uint32 vcpu, 8:uint64 memoryBytes, 9:uint64 diskBytes, 10:StateSeconds seconds, 11:double vcpuHours, 12:double ramGibHours, 13:double diskGibHours, 14:int64 costMicro, 15:bool complete`.
- `StateSeconds`: `1:uint64 running, 2:uint64 standby, 3:uint64 hibernated, 4:uint64 stopped`.
- SnapshotInfo relevant fields: `1:string snapshotId, 2:string name, 3:uint64 version, 4:string status, 7:uint64 sizeBytes, 8:int64 updatedAt, 9:uint32 vcpu, 10:uint64 memoryBytes, 11:uint64 useCount, 12:int64 createdAt`.

Source: `_generated/definitions.js` methods at lines 19440 onward and per-message encoders; `machines.js`, `files.js`, `models.js`, `snapshots.js`.

## Preserve upstream provider semantics

- Create from snapshot accepts restored machine state, not image/env/cmd/network reconfiguration; snapshot size is inherited, then resize separately if required.
- Existing adapter creates managed snapshots with `isolated:true`, disables RAM auto-suspend explicitly, configures auto-hibernate separately, and verifies source snapshot version.
- Custom snapshots use their configured native authentication/isolation semantics; do not silently treat them as managed templates.
- Resume branches by provider state: hibernated → WakeVm; suspended → ResumeVm; stopped → StartVm. Failed is not healthy; destroyed is absent.
- Preserved wake retains runtime processes; stopped/resize needs readiness polling and runtime preparation with process continuity marked lost.
- Pause means HibernateVm followed by bounded inspection; a successful request alone does not establish the final state.
- Deterministic name/conflict reconciliation belongs to allocation; do not create a second workspace on an ambiguous response.
- Usage is org-owner/admin scoped and can include deleted machines. Upstream validates machine ID, complete=true, currency `usd`, exact requested second-rounded bounds, nonnegative integer costMicro.
- Keep usage evidence and provider settlement separate from frontend estimates; never substitute a hardcoded hourly rate for provider evidence.
- Complete parity additionally needs snapshots, fork isolation, egress controls, proxy create/list/delete, supervised process ownership and provider readiness checks from the original adapter.

Source: [frozen Boxd adapter](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/packages/sandbox-providers/src/boxd.ts#L480), especially allocation 480–575, resume 838–899 and usage 1080–1115.

## E2B direct HTTP alternative

These contracts are implemented by the frozen adapter itself; no E2B SDK is required for its existing path.

| Operation | Exact request |
| --- | --- |
| Create | `POST https://api.e2b.app/sandboxes`; `x-api-key`, JSON `{templateID,timeout,secure:true,metadata,envVars,autoPause,...networkFields}` |
| Inspect | `GET /sandboxes/:id`; response contains `sandboxID,state,domain?,envdAccessToken?,metadata?` |
| Resume/connect | `POST /sandboxes/:id/connect`, JSON `{timeout,autoPause}`; obtain refreshed envd token/detail |
| Pause | `POST /sandboxes/:id/pause`, JSON `{memory:true}`; original tolerates 409 |
| Timeout | `POST /sandboxes/:id/timeout`, JSON `{timeout}` |
| Network | `PUT /sandboxes/:id/network`; open/quarantine uses `allow_internet_access`, restricted uses `allowOut,denyOut` |
| Snapshot | `POST /sandboxes/:id/snapshots`, JSON `{name}`; response `{snapshotID}` |
| Delete | `DELETE /sandboxes/:id`; original tolerates 404 |
| Bootstrap exec | `POST https://49983-<id>.<domain>/process.Process/Start` using Connect JSON framing |
| Upload | `POST https://49983-<id>.<domain>/files?path=<encoded>`, multipart field `file`; optional username query |
| Read | `GET` same files URL; `filesystem.Filesystem/Stat` handles existence |

- Create response supplies `sandboxID`, optional `domain` and `envdAccessToken`; cache only within the same API origin/credential scope.
- Guest headers: `E2b-Sandbox-Id:<id>`, `E2b-Sandbox-Port:49983`, `X-Access-Token:<envd token>`; optional `Authorization: Basic base64(user + ":")` for process/stat user.
- Exec headers additionally `Connect-Protocol-Version:1`, `Content-Type:application/connect+json`.
- Exec payload: `{process:{cmd,args,cwd,envs},tag,stdin:false}`, UTF-8 JSON preceded by flag zero and four-byte big-endian length; parse Connect envelopes/events and end-stream errors, not grpc-web trailer text.
- On guest 401/403, clear scoped credential cache and refresh once. Reading old history must not call provider connect merely to obtain a token.
- E2B path reduces codec work but does not prove Boxd-equivalent preserved-process behavior; frozen adapter returns processContinuity unknown on resume.
- Frozen E2B adapter does not implement a Boxd-style GetVmUsage method; billable settlement needs existing E2B usage-source integration, not guessed lifecycle-duration charges.

Source: [frozen E2B adapter](https://github.com/swarajbachu/zuse/blob/2016cf4f0cc240e6797916dd5cab1d8bebab89ae/packages/sandbox-providers/src/e2b.ts), especially 255–282, 350–378, 400–525, 648–722 and 810–873; API `cloud-billing-usage-sources`.

## Licensing and implementation handoff

- Published SDK package LICENSE is MIT, copyright 2026 Azin. Preserve notice/license in THIRD_PARTY_NOTICES and any vendored/generated derivative protobuf subset.
- Generated files identify protoc-gen-ts_proto 2.11.6 / protoc 3.19.1, source `api.proto`; the package contains generated code, not the raw `.proto`.
- Record the npm package/version/tarball integrity from `.context/boxd-sdk-reference/npm-metadata.json` alongside any extracted schema; do not label reconstructed definitions provider-authored raw proto.
- Original application adapter reuse must retain the original repository license/notice obligations independently of the MIT SDK.
- First implementation slice: token exchange cache → bounded grpc-web unary → snapshot/get/create/start/wake/hibernate/destroy → exec/upload bootstrap → usage decode, then additional image/fork/proxy methods.
- Confirm request encoding with SDK-generated byte fixtures before live provider use; test split frames, trailers-only auth errors, nonzero grpc-status, large integers, cancellation and ambiguous creates.
- No live compatibility, runtime boot, provider-account permissions, billing settlement or latency was verified in this research.
