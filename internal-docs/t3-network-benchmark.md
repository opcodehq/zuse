# Zuse vs T3 Code: startup and chat benchmarks

Open the [interactive HTML report](zuse-vs-t3-3g.html) for comparison bars, individual samples, chat and agent runtime results, methodology and downloadable measurement data. The report works offline.

T3 Code loads faster on a first visit; Zuse reloads faster with a fully warmed cache. In the matched chat fixture, Zuse receives history and first text sooner and resumes sooner after reconnect. T3 loads older history and stops faster. Zuse now transfers fewer bytes during text streaming and tool traces. These are local artifact and fixture measurements, not remote model inference speeds.

Both apps used Chrome for Testing 151.0.7922.34, a 1280 × 1000 viewport, 400 ms emulated latency, 400 kbps download and 200 kbps upload. Values below are medians of three samples per app per metric. Workspaces were empty, authentication was established before measurement, and T3 onboarding was completed.

| Metric | Zuse | T3 Code 0.1.7 |
|---|---:|---:|
| Cold usable UI | 76.9 s | 64.3 s |
| Fully cached reload | 4.7 s | 23.1 s |
| Cold HTTP transferred | 3.72 MB | 2.89 MB |
| Cached reload HTTP transferred | 1.3 KB | 85.8 KB |
| Cold same-origin responses | 10 | 290 |

Zuse's cold range was 76.6–81.4 seconds; T3's was 64.3–64.6. Warm ranges were 3.8–5.1 seconds for Zuse and 23.1–23.2 for T3.

T3's shipped local server returned `Cache-Control: no-cache` with ETags for its hashed entry JavaScript and Inter font. Zuse's fingerprinted assets use immutable caching. This policy difference likely explains much of the reload gap; these timings do not establish an intrinsic rendering advantage. A CDN, HTTP/2 reverse proxy or different cache headers can change the results.

Cold HTTP bytes are encoded responses captured through UI readiness plus one second, including headers. They are not the entire distribution size and exclude WebSockets and unfinished background transfers. Both apps negotiated Brotli. Fully warmed reloads used separate profiles prepared at unrestricted network speed until HTTP requests settled, then restored 3G throttling before timing.

Initial T3 trials accidentally included its setup dialog; those T3 measurements were excluded and repeated. Valid Zuse cold samples were retained. Screenshots and complete sample data are preserved for review. No real provider or paid model turn was run. The matched chat follow-up below measures streaming, Stop and large history under a common fixture. Upload timing remains a Zuse-only smoke check.

The immediate remaining network target for Zuse is its first-load WASM payload: 3.69 MB with Brotli. T3 reached its usable empty workspace with less HTTP data.

## Matched chat and agent runtime

Three paired samples per app, alternating execution order, use the same 1,500-message history with alternating user/assistant roles and exactly matching canonical text. Both use the same local Claude-protocol fixture through their real provider adapters. It emits 40 text chunks 50 ms apart, with completed paragraphs every five chunks, and three distinct 32,794-byte tool results. Real browser controls open history, send prompts, reconnect and Stop. No external model calls occur.

| Metric, median n=3 | Zuse | T3 Code 0.1.7 |
|---|---:|---:|
| Initial chat history received | 0.521 s | 1.095 s |
| Older history received | 0.736 s | 0.469 s |
| Send to first text received | 0.792 s | 1.641 s |
| Stop to provider interruption | 0.839 s | 0.379 s |
| Live text resumes after disconnect | 2.460 s | 5.195 s |
| Chat opening, encoded download | 1,561 B | 2,675 B |
| Older history, encoded download | 1,380 B | 1,510 B |
| Text response, encoded socket download | 4,968 B | 7,613 B |
| Tool trace, encoded socket download | 3,980 B | 8,869 B |
| Reconnect, encoded socket download | 9,092 B | 16,526 B |
| Three compact routine tool records, decoded JSON | 901 B | 2,857 B |
| One complete tool output fetched on demand | 32,794 B | 32,794 B |

The HTML includes all samples, ranges and definitions. Initial receipt and first text measure browser receipt, not paint. Zuse opens 32 messages and retains 64 after requesting 32 missing older messages; T3 opens 20 and fetches a 40-message older page. These compare each product's default behavior, not equal page sizes. Both retain all 1,500 messages. History is imported through Zuse's native importer; T3's stopped disposable projection database is seeded directly, then its native APIs and retained message count are verified.

The runtime application shell loads without throttling. A loopback proxy then meters aggregate **encoded TCP bytes**, preserving order and applying 50,000 B/s down, 25,000 B/s up, 400 ms HTTP latency and 200 ms delay in each socket direction. Native T3 permessage-deflate and HTTP Brotli remain enabled; Zuse negotiates per-connection dictionary-preserving deflate for direct browser frames. Decoded JSON is never used as a substitute for network bytes. Opening and older-history totals include the relevant encoded HTTP response when used; reconnect's chart counts socket bytes, with shell HTTP bytes listed separately in the raw data.

All six samples verify every text chunk, three distinct tool-output SHA256 hashes through separate on-demand reads, an agent continuing through reconnect, exactly three provider starts and one interruption, and no replayed execution. The fixture is highly repetitive and compressible. It does not cover physical radios, mobile CPU, packet loss, concurrent active agents, large sidebar populations, matched attachment uploads, matched silent-link recovery, or relay/account transport. Do not generalize its byte ratios to arbitrary transcripts.

**Artifact qualification:** Zuse's browser is built with `web-release`, but the native engine is `dev (debug)`; T3 is the shipped 0.1.7 binary. This is not a release-to-release CPU benchmark. Exact WASM SHA256 and server build metadata are recorded in `chat-results.json`. The startup comparison used an earlier browser build and was not repeated after the chat changes.

## What the T3 release source does

The source review now includes the exact measured release commit `322f6544dfad0a3c8a0c8cbecf2c7f7874b14268`, supplementing the initial review of `572cf74275bc0fc4dcdaee754db64fb3d5b575c1`.

- `server.ts` enables permessage-deflate with context takeover; `http.ts` compresses HTTP responses. Zuse now negotiates dictionary-preserving deflate frames on the direct browser socket, uses native bounded browser decompression, and keeps decoding cancellation-safe while outgoing actions proceed.
- `WireProjection.ts` omits command output and successful file-diff bodies from routine updates; full details remain available through `getTurnItem`. Zuse already has tool blob references and paged reads. Both passed the output-integrity fixture.
- `orchestrationV2DedupedBodies.ts` deduplicates repeated bodies and compact records/ID tables reduce envelope repetition. Zuse already sends incremental transcript deltas and now omits unchanged history metadata while preserving the client's pagination state.
- `assistantStreaming.ts` uses paragraph boundaries and a 400 ms gate; `ThreadLiveEventCoalescer.ts` coalesces replaceable events over 50 ms. Zuse keeps incremental text delivery. The current Zuse dictionary codec uses fewer encoded bytes in the measured text stream.
- `threadHistoryPaging.ts` bounds opening history by 10 user turns, 75 outside-turn items, up to 500 turn items and 1 MiB. It pages older history and defers large work sections. Zuse's opening is bounded, and verified cached-history hints avoid resending an unchanged suffix and a single intact oversized message can exceed its budget.
- `ShellStream.ts` can omit idle subagents and older settled threads for capable clients. `LiveStreamBudget.ts` bounds queued and in-flight traffic by 1,000 items and 8 MiB until acknowledgment. Zuse now sends keyed list deltas and bounds queued/in-flight browser output by bytes. Initial selective sidebar loading remains an opportunity; this single-chat fixture does not measure them.

## Reproduce the chat comparison

Use Node 22+, a WebGPU-capable Chromium, matching `dist/web` and `target/debug/zuse-server`, and the disposable T3 0.1.7 binary/profile under `.context/t3-benchmark`. Never point the fixture at real user profiles. The runner creates its own projects, provider CLI, logs, databases and authentication sessions.

```sh
CHROME_BIN=/path/to/chrome BENCH_RUNS=3 \
  node scripts/acceptance/compare-chat.mjs
python3 scripts/acceptance/render-chat-report.py
```

Raw samples are in `.context/t3-benchmark/chat-results.json`; screenshots and wire evidence are under its recorded artifact directory. The renderer embeds sanitized sample data in the offline HTML. The earlier Zuse-only upload and silent-link checks remain in `performance-baseline.md`.

## Builds and evidence

- T3: shipped `t3 v0.1.7`, release tag commit `322f6544dfad0a3c8a0c8cbecf2c7f7874b14268`. The installer verified the release SHA256SUMS. The exact release source was exported and inspected alongside this binary.
- Zuse: commit `abd74d19c6f3acfd3308b05ad141928e71276439` plus the uncommitted slow-network optimizations, built with `scripts/build-web.sh --release`.
- Startup raw results, sample assets, build identities and measurement definitions: `.context/t3-benchmark/results.json`.
- Browser screenshots: the artifact paths recorded in `.context/t3-benchmark/comparison-3g.json`, `t3-corrected-3g.json` and `settled-warm-3g.json`.
- Reusable runner: `scripts/acceptance/compare-startup.mjs`. It requires Node 22+, `CHROME_BIN`, and `T3_BENCH_AUTH_FILE` pointing to a private JSON file containing the disposable T3 origin and its Set-Cookie strings. `BENCH_RUNS` defaults to 3; `BENCH_MODE=warm-only` measures independently prewarmed reloads. Never use real user profiles for this test.

The current runtime uses dictionary compression, keyed chat/session deltas and verified history reuse. Stop is measured after an accepted prompt has been acknowledged before disconnect; the earlier batch did not explicitly await that acknowledgment. The report’s before/after batches are descriptive and do not isolate each change.
