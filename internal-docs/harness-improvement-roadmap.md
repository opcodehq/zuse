# Native harness improvement priorities

The source audit and measured coding failures point to agent/tool interaction and
observability before another runtime rewrite. This roadmap complements
[harness-tool-efficiency.md](harness-tool-efficiency.md). Exact pinned reference
source links and the 14-area parity matrix are retained in the private research
artifact `.context/round-trip-followup/reference-research.md`.

## What already exists

The application already shares native execution services, a model broker and HTTP
transport across roots. It has native tools, task-plan events/UI, account affinity,
bounded outputs, exclusive root leases, checksummed journals, artifacts and
uncertain-tool-outcome review. Basic text/image handling, ripgrep search and child
messaging also exist. These are not reasons to rebuild the harness or reintroduce
an SDK subprocess.

Protocol coverage is not provider/authentication parity. Four generic wire
protocols do not prove every subscription login, endpoint, model, reasoning mode
or provider-specific continuation works. The actual live coverage and failures
must remain visible.

## Delivery order

| Priority | Change | Concrete scope and source of truth | Acceptance |
| --- | --- | --- | --- |
| First, implemented | Reduce mistaken tool use and lengthy routine responses | Schema-owned native workflow guidance, actionable create-conflict errors, early rejection before replacement IO. See the measured follow-up. | Existing permissions/CAS tests pass; coding acceptance and final completion both count. Small new probes do not replace the original full-suite results. |
| Next | Stage-level latency and cache diagnostics | Extend existing request events with first tool/text delta, provider wait, tool execution, storage/admission and final generation timing. Explain likely cache misses from existing account/model/config identities and actual usage. | Tool-only responses have a meaningful first-content time; no payload/token logging, new probe requests, duplicate usage accounting or misleading cache-hit inference. |
| Next | Matched shared-host measurement | One backend per arm at 1/2/5/10/20 roots; first-root and incremental-root PSS, idle/active/closed memory, CPU, retained disk, IO and recovery. Keep isolated CLI results as a separate topology. | Compare equivalent lifecycles and tools. Sum concurrent PSS as well as RSS; include children. Do not interpret summed RSS as physical memory or compare one host against many processes. |
| Next | Bounded independent reads | Use the existing tool registry, pending calls and durable receipts; opt in proven independent file/search operations with a small shared limit. | Persist starts before execution, commit every receipt before acknowledgement, retain deterministic history and cancellation/recovery behavior. Do not parallelize by `ToolCategory::Read` alone: questions and plan updates also use that category and have observable effects. Keep dependent calls and mutations ordered. |
| Next | Provider conformance and clear capability failures | Route-specific tests for model/effort/images/tools/cache/auth, account switches, renewal, disconnects and continuation signatures; explicit subscription and API profiles. | No silent reasoning changes, fallback models or paid-route substitution. Keep unsupported combinations explicit. Verify special subscription endpoints rather than inferring them from public API protocol support. |
| Optional | Generic third-party MCP | Reuse app tool infrastructure, adding a client pool keyed by server configuration, credential/account and permission scope. Load schemas lazily; use bounded startup, cancellation and idle expiry. | Preserve built-in authenticated app routing. No external server starts for an unused capability; no cross-account or cross-permission reuse. Native integration currently targets the built-in endpoint rather than a generic external pool. |
| Optional | Structural search and skill loading | Reuse available syntax/plugin infrastructure for bounded symbol/outline queries and a small skill metadata inventory with explicit on-demand content loading. | Invalidate search results by actual file version. Preserve instruction priority; avoid another full index, per-turn directory scans, blanket skill injection or default context trimming. |
| Optional | Peer-edit awareness | Coalesced file-version notices derived from shared checkout/read receipts and existing application events. | Notify relevant readers only, bound prompt traffic, retain hash checks and one event authority. Do not start a filesystem watcher or process for every root. |
| Later, measured need | Large-history/transport specialization | Lazy presentation or idle-root hydration, selective provider-supported request compression and persistent transport where supported. | Keep canonical durable history recoverable and active/recovery references alive. Measure CPU/cache rebuilds and disconnect/cancellation behavior; no unconditional cache warming or default summary traffic. |

## Architecture constraints

Use one authoritative state and registry for each concern. The basic plan already
travels through native tool events into the document/UI projection; an empty plan
callback does not justify adding another plan store. Stable tool definitions and
instructions should change deliberately. Recovered roots keep their saved system
instructions; instruction refresh needs an explicit versioned transition rather
than silent transcript rewriting.

Keep the existing fresh-hash mutation contract, containment and root ownership.
Better tool ergonomics must not weaken these guarantees. Retry uncertain side
effects only after the existing review policy permits it. Optional document,
embedding, cloud-provider and UI capabilities should not become mandatory core
dependencies just to match a feature list.

Preserve the selected model and effort and the full-history fallback. Any future
context reduction requires the existing quality gate, including long histories,
conflicting instructions, tool receipts, large outputs and recovery. Success is
measured accepted-and-completed tasks per resource/token budget, including retries,
summaries, children, failures and capped runs. Subscription usage is not a dollar
invoice.
