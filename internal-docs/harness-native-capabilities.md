# Native harness capabilities and operation

Select **Zuse** as the harness in the application, connect a supported model account,
and select the model and reasoning level. The application runs the Rust kernel
with shared native HTTP transport and owned file/process tools. External coding
CLIs do not run on this kernel. Native execution requires no Node bridge.
API-only profiles remain explicit; see [provider configuration](harness-efficiency.md).
A working wire adapter does not establish live access for every model/account.

## Scheduling and orchestration

The root owns integration and validation. Children receive a concrete task and
separate file ownership; analysis children can inherit read-only mode. Independent
children can run concurrently, exchange durable messages and return outcomes to
the parent. Depth remains bounded to two. Model, reasoning, permission inheritance
and the shared request budget remain authoritative across the graph.

Optional profile-owned `<profile>/harness/runtime.json` configures admission:

```json
{"parallelReads":4,"children":3}
```

`parallelReads` accepts 1–16; `children` accepts 1–32. Missing settings use the
values above. Unknown fields, invalid values and files over 4 KiB fail explicitly.
Restart the backend after editing settings. Increasing concurrency can increase
memory, provider rate pressure and model requests; it does not expand the budget.

Only explicitly approved native reads can overlap, up to the per-agent limit and
a process-wide limit of 16. Blocking file work has its own shared bound of 16,
retained until the worker exits even if its caller cancels. Questions, plans,
edits, process execution, external calls and delegation retain ordered execution.
Sensitive permission prompts are serialized per native host. All batch starts
commit before execution; completed receipts commit as reads finish; model history
receives results in original call order. An interrupted started receipt still
requires inspection before retry. Concurrency does not infer dependencies: a
model must request dependent work in later turns.

## Local skills and structural navigation

`list_skills` discovers at most 64 metadata entries under
`<checkout>/.agents/skills/<name>/SKILL.md`; `read_skill` loads one body on demand,
up to 128 KiB. Skill names are bounded safe directory names. Traversal, external
paths and sensitive aliases are rejected. Bodies are not automatically injected
into each prompt. Returned hashes identify the actual file version.

`code_outline` parses Rust, Python, JavaScript/JSX and TypeScript/TSX with
Tree-sitter. It returns bounded names, kinds, signatures and line ranges with the
fresh content hash and a parse-error indicator. It does not build a background
index. Source files retain the existing 2 MiB limit; results have at most 512
symbols and traversal is bounded. Disable the `structural-search` crate feature
for a minimal embedding that does not need parsers.

Native file reads record bounded version observations. Successful native writes
notify relevant peer readers in the same checkout, coalescing at most 16 paths
per recipient. Notices enter the existing durable mailbox and are acknowledged
after persistence. They are hints; fresh hashes still govern every mutation.
These notices cover native edit tools, not shell commands or arbitrary external
editors. They add no watcher process and do not replace the application watcher.

## External MCP tools

The application loads `<profile>/harness/extensions.json` once on backend startup.
Configuration belongs to the trusted profile, never to repository instructions.
No server is started until its capability is used. Example:

```json
{
  "servers": {
    "docs": {
      "trusted": true,
      "protocol": "2025-06-18",
      "readOnlyTools": ["search"],
      "connection": {
        "type": "stdio",
        "executable": "/absolute/path/to/server",
        "args": ["--stdio"],
        "env_refs": {"SERVICE_TOKEN": "DOCS_SERVICE_TOKEN"}
      }
    },
    "remote": {
      "trusted": true,
      "protocol": "2026-07-28",
      "connection": {
        "type": "streamable-http",
        "url": "https://tools.example.com/mcp",
        "token_env": "REMOTE_MCP_TOKEN"
      }
    }
  }
}
```

Use `mcp_servers`, then paginated `mcp_tools`, then `mcp_call`. The small stable
native schemas avoid injecting every remote tool schema into each request.
Supported configured protocol versions are 2025-06-18 (default), 2025-11-25 and
2026-07-28; modern discovery is explicit. HTTP requires HTTPS except loopback;
redirects and endpoint changes are rejected. Remote annotations never grant
read-only permission: only `readOnlyTools` in the trusted profile can narrow a
call's category. Unknown external calls retain network permission requirements.

The shared pool has at most eight connections, four in-flight requests per
connection. Connections become eligible for idle retirement after 60 seconds; a
shared janitor checks every 30 seconds. It isolates checkout, configured
credentials and effective permission scope; credential changes create a distinct
connection. Stdio children receive a small base environment plus explicit
references, not inherited model keys. Trusting a server authorizes a process
with the backend user's OS access; permission categories are not OS isolation.
Children have owned process groups that terminate on eviction/shutdown. No
server process or HTTP client is allocated for an unused server.

Startup is capped at 12 seconds with a 30-second failed-start cooldown; operations
are capped at 30 seconds. Configurations are limited to 128 KiB/32 servers,
stdio lines to 1 MiB, discovery to 256 tools/4 MiB per page, arguments to 1 MiB,
and results/HTTP streams to 4 MiB. Oversized responses fail explicitly.
SDK result caching and automatic retries/reinitialization are disabled. Lost,
timed-out or unsupported task/interaction outcomes pause for review, rather than
repeating a possibly completed side effect. A reviewed later operation can open
a fresh connection. Sampling, autonomous remote tasks and elicitation are not
implemented. Text and images retain structured results on image-capable native
protocols; incompatible routes fail explicitly.

## Diagnostics and evaluation

Each physical model attempt exposes optional admission, preparation, response
header, first-content, first-text and stream timings. Tool-only streams have a
first-content time. Cumulative tool time/count and storage time are marked as
cumulative totals, not per-request costs. Errors/cancellation retain partial
measurements where available. Existing physical request/usage accounting remains
the source of truth.

A bounded local monitor reports account/model/configuration/history changes or
an append-only prefix. It stores hashes and identifiers, not prompt content.
These explain possible cache invalidation; only provider-reported cache counters
establish hits. Unknown counters remain unknown. Explicit API routes can declare
`allowedReasoning`; unsupported requested effort fails before dispatch.

`shared-host` and `scripts/benchmark-shared-host.py` measure idle shared backends
at 1/2/5/10/20 roots without sending prompts. The private manifest supplies local
server controls. Results include settled process-tree RSS/PSS and native
post-close memory. This is a core idle measurement with different feature
surfaces, not full desktop, active coding throughput or live task equivalence.
See [measured implementation results](harness-roadmap-results.md).

Existing recovered roots retain their saved instructions. No default context
reduction, model downgrade or cache-warming traffic is introduced. New live
speed, completion-rate and cost claims require a separately authorized request
budget and matched acceptance tasks; the existing global 300-request ledger is
exhausted and remains unchanged. The subsequently authorized
[2026-10-28 full live rerun](harness-followup-2026-10-28.md) used its own 201/300
follow-up allowance and publishes the current measured outcomes.
