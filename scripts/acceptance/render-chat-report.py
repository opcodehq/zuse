#!/usr/bin/env python3
"""Update the self-contained report only from complete matched measurements."""
import json
import re
import statistics
from pathlib import Path

report_path = Path(".context/t3-benchmark/chat-results.json")
html_path = Path("internal-docs/zuse-vs-t3-3g.html")
report = json.loads(report_path.read_text())
assert not report.get("failure"), "Refusing to publish an incomplete benchmark"
for app in ("zuse", "t3"):
    samples = [s for s in report["samples"] if s["app"] == app]
    assert len(samples) >= 3 and all(all(s["checks"].values()) for s in samples)

metrics = [
    ("chatOpenMs", "Initial chat history", "time", "ms", "Receipt of the first bounded snapshot; Zuse shows 32 messages and T3 20 by default."),
    ("olderHistoryMs", "Load older history", "time", "ms", "Zuse validates its cached 32-message suffix and normally transfers only 32 older messages; T3 requests 40 older messages. A stale Zuse suffix falls back to a reset."),
    ("sendToFirstAppendMs", "First streamed text reaches browser", "time", "ms", "Socket receipt, not a renderer paint measurement. Same CLI fixture: 40 chunks, 50 ms apart, completed paragraphs every five chunks."),
    ("stopAcknowledgedMs", "Stop reaches the agent", "time", "ms", "Real Stop control after reconnect; completion is the provider's interruption log."),
    ("runningAgentReconnectMs", "Live text resumes after disconnect", "time", "ms", "The existing long-running fixture resumes; provider execution counts exclude duplicate starts."),
    ("openingWireBytes", "Chat opening · transferred bytes", "network", "bytes", "Socket bytes during opening plus the encoded HTTP bounded response, when used."),
    ("olderWireBytes", "Older history · transferred bytes", "network", "bytes", "Socket bytes plus the encoded HTTP history page. Products use different default page sizes."),
    ("streamWireBytes", "Text response · socket bytes", "network", "bytes", "Actual encoded socket download for the 40-chunk response, including lifecycle events."),
    ("toolWireBytes", "Tool trace · socket bytes", "network", "bytes", "Three 32,794-byte tool results and the same text response; full outputs are fetched separately for integrity checks."),
    ("reconnectWireBytes", "Reconnect · socket bytes", "network", "bytes", "Socket download including resumed events and upgrade headers. HTTP shell bytes are listed separately in raw data."),
    ("routineToolOutputBytes", "Three compact tool records", "details", "bytes", "Decoded JSON for three routine tool records. Both preserve complete output behind an on-demand read."),
    ("toolDetailsOnDemandBytes", "One full tool output, on demand", "details", "bytes", "Both return all 32,794 bytes; SHA256 matches the fixture source."),
]

for sample in report["samples"]:
    def http_bytes(path_suffix):
        return sum(x["encodedBytes"] for x in sample["httpTransfers"] if x["url"].endswith(path_suffix))
    sample["openingWireBytes"] = sample["phases"]["open"]["encodedWsDownloadBytes"] + http_bytes("/bounded")
    sample["olderWireBytes"] = sample["phases"]["older"]["encodedWsDownloadBytes"] + http_bytes("/history")
    sample["streamWireBytes"] = sample["phases"]["stream"]["encodedWsDownloadBytes"]
    sample["toolWireBytes"] = sample["phases"]["tools"]["encodedWsDownloadBytes"]
    sample["reconnectWireBytes"] = sample["phases"]["reconnect"]["encodedWsDownloadBytes"]
    sample.pop("artifacts", None)

report["metrics"] = [dict(zip(("key", "title", "group", "unit", "note"), row)) for row in metrics]
report["summary"] = {}
for app in ("zuse", "t3"):
    report["summary"][app] = {}
    for key, *_ in metrics:
        values = [s[key] for s in report["samples"] if s["app"] == app]
        report["summary"][app][key] = {"median": statistics.median(values), "min": min(values), "max": max(values), "n": len(values)}
report.pop("artifacts", None)
report["sourceArtifact"] = str(report_path)
html = html_path.read_text()
data_match = re.search(r'<script type="application/json" id="benchmark-data">(.*?)</script>', html, re.S)
data = json.loads(data_match.group(1))
data["matchedChat"] = report
encoded = json.dumps(data, ensure_ascii=False, separators=(",", ":")).replace("<", "\\u003c").replace(">", "\\u003e").replace("&", "\\u0026")
html = html[:data_match.start(1)] + encoded + html[data_match.end(1):]
html = re.sub(r'<p class="intro">.*?</p>', '<p class="intro">Real measurements of startup, chat history, streaming, Stop, tool output and reconnect. Both products use the same 1,500-message history and local provider fixture for the chat comparison.</p>', html, count=1, flags=re.S)
html = re.sub(r'<div class="notice">.*?</div>', '<div class="notice"><strong>Now measured head to head:</strong> chat opening, older history, first text, Stop, live-agent reconnect, transferred bytes and tool-output retrieval. Chat charts show three paired runs per app using encoded-byte 3G throttling. Startup charts retain the earlier independent three-run benchmark. These are local fixture results, not remote model inference times.</div>', html, count=1, flags=re.S)
runtime = '''<section id="runtime"><div class="section-head"><div><div class="section-title"><span class="section-no">02</span><h2>Chat and agent runtime: head to head</h2></div><p class="subtitle">Same 1,500 messages · alternating user / assistant · 3 runs per app · medians and ranges</p></div><span class="tag matched">Both apps measured</span></div>
<div class="toolbar"><div class="tabs" role="tablist" aria-label="Chat comparison metric"><button role="tab" id="chat-tab-time" aria-controls="matched-charts" aria-selected="true" data-chat-view="time">Action time</button><button role="tab" id="chat-tab-network" aria-controls="matched-charts" aria-selected="false" tabindex="-1" data-chat-view="network">Actual network bytes</button><button role="tab" id="chat-tab-details" aria-controls="matched-charts" aria-selected="false" tabindex="-1" data-chat-view="details">Tool data retained</button></div><div class="legend"><span><i></i>Zuse</span><span class="t3"><i></i>T3 Code</span></div></div>
<div class="chart-grid" id="matched-charts" role="tabpanel" aria-labelledby="chat-tab-time" aria-live="polite"></div>
<p class="caption">Every chart starts at zero. Each metric has its own common scale for both apps. Lower time or transfer is better; equal on-demand output sizes confirm retention. Small n=3 ranges describe these builds, not confidence intervals.</p>
<div class="paired-explain"><article class="plain-card"><h3>Compression is counted correctly</h3><p>The proxy throttles encoded TCP bytes at 50,000 B/s down and 25,000 B/s up, with 400 ms socket RTT. T3's native permessage-deflate and HTTP Brotli are counted after compression. Zuse's opt-in dictionary-preserving deflate frames are counted the same way. Decoded JSON sizes are available separately.</p></article><article class="plain-card"><h3>Same source history, different windows</h3><p>Both retain all 1,500 messages, with identical text and roles verified. T3's first window contains 20 messages; Zuse's contains 32. T3 loads only the next older page. Zuse normally sends only the missing prefix after verifying the cached suffix SHA256. These compare actual default behavior, not identical window sizes.</p></article></div>
<div class="plain-card" style="margin-top:16px"><h3>Full flow verified in both products</h3><div class="check-grid"><span class="passed">Initial history and older messages</span><span class="passed">All 40 text chunks received</span><span class="passed">Three bulky tool results omitted from routine records</span><span class="passed">Full output retrieved with exact SHA256 match</span><span class="passed">Agent keeps running through reconnect</span><span class="passed">Stop reaches the same provider process</span></div><p class="small">Four hundred milliseconds of explicit socket RTT is applied separately from HTTP response latency. Both use the same fixture binary; SDK and engine launch paths remain product-specific. No external model call is made.</p></div>
</section>'''
html = re.sub(r'<section id="runtime">.*?</section>', lambda _: runtime, html, count=1, flags=re.S)
# Remove the superseded Zuse-only chart and table renderers, retaining startup.
html = re.sub(r'function renderRuntime\(\).*?(?=const r=data\.runtime\.silent;)', '', html, count=1, flags=re.S)
html = re.sub(r"let table='';.*?\$\('comparison-body'\)\.innerHTML=table;", '', html, count=1, flags=re.S)
html = html.replace('renderStartup();renderRuntime();', 'renderStartup();')
html = html.replace('Missing measurements stay visible. Startup uses three-sample medians; runtime shows individual fixture runs.', 'Startup and chat use independent three-sample medians. All chat metrics below are measured in both products.')
html = html.replace('Before / after comparisons use Zuse\'s own baseline, independently of T3.', 'The same current 1,500-message fixture: complete history, recent window, then compressed transfer. These are Zuse before / after payloads.')
html = html.replace('Same 1,502-entry chat · first history response', 'Same 1,500-message fixture · one initial history response')
html = html.replace('<h3>Zuse runtime follow-up</h3>', '<h3>Matched chat follow-up</h3>')
html = re.sub(r'(<h3>Matched chat follow-up</h3>)<ul>.*?</ul>', r'\1<ul><li>Three paired runs per product, alternating which app runs first.</li><li>Same 1,500 alternating user/assistant messages; exact canonical text and roles verified.</li><li>Zuse history is imported; T3 history is inserted into a stopped disposable projection database. The two seed paths differ.</li><li>Real composer, history buttons, provider adapters, Stop and reconnect. Forty chunks at 50 ms intervals; completed paragraphs every five chunks.</li><li>Native socket compression remains enabled. The loopback proxy meters actual encoded bytes, preserves TCP order and applies shared directional rate limits.</li><li>Application shell downloads are excluded from runtime. Desktop CPU, no packet-loss or mobile-radio simulation.</li></ul>', html, count=1, flags=re.S)
html = html.replace('There is no matching T3 runtime fixture was measured;', 'Matching T3 runtime fixtures were measured;')
html = html.replace('No matching T3 runtime fixture was measured; there is no measured comparison of model intelligence, inference time, provider reliability or full platform parity.', 'Both products passed the matched chat fixture. Model intelligence, external inference time, physical radio behavior and full platform parity remain outside this benchmark.')
html = html.replace('Thirty-eight targeted Rust tests passed, with one additional existing RPC test ignored.', 'Compression validation passed three JavaScript cases, two focused transport tests, four history-window tests and seven transcript UI tests. The complete serve suite passed 30 tests; existing RPC coverage includes one ignored test.')
html = html.replace('T3 source review:</b> separate snapshot <code>572cf74275bc0fc4dcdaee754db64fb3d5b575c1</code>, not the measured release.', 'T3 source review:</b> exact measured release <code>322f6544dfad0a3c8a0c8cbecf2c7f7874b14268</code>. The earlier review of snapshot <code>572cf74275bc0fc4dcdaee754db64fb3d5b575c1</code> has been supplemented by release-source inspection.')
reproduction = 'The matched chat runner is <code>scripts/acceptance/compare-chat.mjs</code>, with <code>BENCH_RUNS=3</code> and <code>CHROME_BIN</code>. Raw results: <code>.context/t3-benchmark/chat-results.json</code>. Rebuild the report with <code>python3 scripts/acceptance/render-chat-report.py</code>.<br>'
html = re.sub(r'(?:The matched chat runner is .*?<br>)+(?=The startup comparison runner is)', '', html, flags=re.S)
html = html.replace('The startup comparison runner is', reproduction + 'The startup comparison runner is')
html = html.replace('Zuse and T3 measured results and missing runtime measurements', 'Zuse and T3 measured startup and chat results')
html = re.sub(r'(<b>Zuse runtime build:</b>).*?(?=</p>)', r'\1 <code>dev (debug)</code> native engine and <code>web-release</code> browser, versus T3’s shipped 0.1.7 build. This measures these actual artifacts; it is not a release-to-release CPU benchmark. The embedded <code>matchedChat.zuseBuild</code> records the WASM SHA256, server size and build timestamp. Startup was not rebenchmarked after these runtime changes.', html, count=1, flags=re.S)
html = html.replace('Older history expansion still resends the visible suffix.', 'Older history normally sends only missing rows; a changed cached suffix triggers an authoritative reset.')
html = html.replace('Release browser and engine builds passed.', 'Browser release and native engine builds passed; the matched runtime uses the debug native engine described above.')
html = html.replace('These are local fixture results, not remote model inference times.', 'These are local fixture results, not remote model inference times. Zuse uses a debug native engine and release browser; T3 uses its shipped build.')
html = html.replace('Compression validation passed three JavaScript cases, two focused transport tests, four history-window tests and seven transcript UI tests.', 'Compression validation passed six JavaScript cases, two focused transport tests, five history-window tests and seven transcript UI tests. Keyed-list integrity and command-tail regression tests also pass; native RPC transport coverage passes 34 tests.')
qualification = ' The synthetic history is highly repetitive and compressible; byte ratios will differ with other content. Matched uploads, silent-link recovery, concurrent active agents, large sidebars and relay/account links remain unmeasured.'
if qualification not in html:
    html = html.replace('Model intelligence, external inference time, physical radio behavior and full platform parity remain outside this benchmark.', 'Model intelligence, external inference time, physical radio behavior and full platform parity remain outside this benchmark.' + qualification)
source_review = '''<section id="t3-approach"><div class="section-head"><div><div class="section-title"><span class="section-no">05</span><h2>What T3 skips, and what Zuse adopts</h2></div><p class="subtitle">Exact 0.1.7 release source, reviewed alongside the actual wire measurements.</p></div></div>
<div class="chart-grid">
<article class="plain-card"><h3>Compress the transport</h3><p>T3 uses HTTP compression and native permessage-deflate with a retained dictionary. Zuse now uses zlib-rs level-3 deflate with a per-connection dictionary and native browser streaming decompression. Ready envelopes are batched without a timer. Small messages stay plain; older clients remain compatible.</p><p class="small"><code>server.ts</code> · <code>http.ts</code> → Zuse <code>browser_io.rs</code> / <code>rpc_compression.js</code></p></article>
<article class="plain-card"><h3>Keep bulky output behind a read</h3><p>T3 omits command output and successful file-diff bodies from routine updates. Both products passed the three-output fixture: all 98,382 source bytes remain available, while routine records contain summaries, metadata or references.</p><p class="small"><code>WireProjection.ts</code> → Zuse already stores tool blobs and fetches their pages on demand.</p></article>
<article class="plain-card"><h3>Avoid sending the same body twice</h3><p>T3 deduplicates bodies shared between messages and turn items, compacts records and can use ID tables. Zuse's transcript already uses append deltas. This change also omits unchanged history metadata; the client retains its existing pagination state.</p><p class="small"><code>orchestrationV2DedupedBodies.ts</code> → Zuse <code>rpc.rs</code> / <code>transcript.rs</code></p></article>
<article class="plain-card"><h3>Coalesce meaningful updates</h3><p>T3 buffers paragraph boundaries, gates streaming updates over 400 ms and coalesces replaceable events over 50 ms. This trades fewer updates for later visible text. Zuse keeps incremental text, suppresses no-op frames and batches already-ready envelopes without adding a paragraph delay.</p><p class="small"><code>assistantStreaming.ts</code> · <code>ThreadLiveEventCoalescer.ts</code></p></article>
<article class="plain-card"><h3>Leave unneeded sidebar data out</h3><p>T3 can omit idle subagents and older settled threads from its first shell snapshot, fetching them when needed. Zuse now sends keyed chat/session list deltas and suppresses unchanged snapshots. Its first list still includes all rows; initial selective sidebar paging remains a gap.</p><p class="small"><code>ShellStream.ts</code> · <code>OrchestrationV2SubscribeShellInput</code></p></article>
<article class="plain-card"><h3>Page old work and bound queues</h3><p>T3 pages older history, defers large work sections and bounds pending live traffic by both items and bytes. Zuse now validates cached history to send only missing rows and applies an 8 MiB queued/in-flight output budget to direct, paired and account browser links, including paired relay sessions. One intact oversized response is admitted exclusively.</p><p class="small"><code>threadHistoryPaging.ts</code> · <code>LiveStreamBudget.ts</code></p></article>
</div><p class="caption">These are design findings, not claims that every T3 mechanism has been ported. Compression was added to Zuse's direct browser link; paired, account and relay framing retain their existing behavior.</p></section>'''
html = re.sub(r'<section id="t3-approach">.*?</section>', '', html, flags=re.S)
html = html.replace('<section id="evidence">', source_review + '\n<section id="evidence">', 1)
html = html.replace('<span class="section-no">05</span><h2>Conditions', '<span class="section-no">06</span><h2>Conditions')
script = r'''<script id="matched-chat-script">
const matched=data.matchedChat;
let chatView='time';
const formatChat=(value,unit)=>unit==='ms'?seconds(value):bytes(value);
function matchedCards(){
 const metrics=matched.metrics.filter(m=>m.group===chatView);
 $('matched-charts').setAttribute('aria-labelledby','chat-tab-'+chatView);
 $('matched-charts').innerHTML=metrics.map(m=>{
  const z=matched.summary.zuse[m.key],t=matched.summary.t3[m.key];
  const equal=Math.abs(z.median-t.median)<0.5;
  const winner=equal?'Equal':z.median<t.median?'Zuse':'T3';
  const max=niceMax(Math.max(z.max,t.max));
  const fmt=value=>formatChat(value,m.unit);
  const samples=app=>$('samples').checked?matched.samples.filter(s=>s.app===app).map(s=>s[m.key]):[];
  return '<article class="chart-card"><div class="chart-header"><div><h3>'+esc(m.title)+'</h3><p class="subtitle">Median · '+z.n+' runs each</p></div><span class="chart-winner '+(winner==='Zuse'?'z':'t')+'">'+(equal?'Equal bytes retained':winner+' lower')+'</span></div>'+axis(max,fmt)+bar('Zuse',z.median,max,fmt,'',samples('zuse'))+bar('T3 Code',t.median,max,fmt,'t3',samples('t3'))+($('samples').checked?'<p class="sample-note">Range · Zuse '+fmt(z.min)+' – '+fmt(z.max)+'<br>T3 '+fmt(t.min)+' – '+fmt(t.max)+'</p>':'')+'<div class="chart-insight">'+esc(m.note)+'</div></article>';
 }).join('');
}
const chatTabs=[...document.querySelectorAll('[data-chat-view]')];
$('samples').addEventListener('change',matchedCards);
function chatTab(button,focus=false){chatView=button.dataset.chatView;chatTabs.forEach(b=>{b.setAttribute('aria-selected',String(b===button));b.tabIndex=b===button?0:-1;});matchedCards();if(focus)button.focus();}
chatTabs.forEach((button,index)=>{button.addEventListener('click',()=>chatTab(button));button.addEventListener('keydown',event=>{let next;if(event.key==='ArrowRight')next=(index+1)%chatTabs.length;if(event.key==='ArrowLeft')next=(index+chatTabs.length-1)%chatTabs.length;if(event.key==='Home')next=0;if(event.key==='End')next=chatTabs.length-1;if(next!==undefined){event.preventDefault();chatTab(chatTabs[next],true);}});});
let comparison='';
const tableGroup=title=>{comparison+='<tr class="table-group"><td colspan="4">'+esc(title)+'</td></tr>';};
const tableRow=(title,z,t,evidence)=>{comparison+='<tr><td>'+esc(title)+'</td><td class="z">'+esc(z)+'</td><td class="t">'+esc(t)+'</td><td class="status">'+esc(evidence)+'</td></tr>';};
tableGroup('Earlier startup benchmark · median n=3');
for(const mode of ['cold','warm']){const z=data.startup.summary.zuse[mode],t=data.startup.summary.t3[mode];tableRow((mode==='cold'?'Cold':'Cached')+' usable UI',seconds(z.medianMs),seconds(t.medianMs),'Both measured');tableRow((mode==='cold'?'Cold':'Cached')+' HTTP transferred',bytes(z.medianHttpBytes),bytes(t.medianHttpBytes),'Encoded HTTP bytes');}
tableGroup('Matched chat fixture · 1,500 identical messages · median n=3');
for(const m of matched.metrics)tableRow(m.title,formatChat(matched.summary.zuse[m.key].median,m.unit),formatChat(matched.summary.t3[m.key].median,m.unit),'Both measured · n=3');
tableRow('Initial message window','32 messages','20 messages','Product defaults differ');
tableRow('Older history transfer','32 missing rows; 64 retained','40-message older page','Product defaults differ');
tableRow('Complete source history retained','1,500 messages','1,500 messages','Canonical text and roles verified');
tableRow('Provider starts / sample','3','3','No duplicate run on reconnect');
tableRow('Complete tool output integrity','SHA256 matches','SHA256 matches','Separate on-demand API check');
$('comparison-body').innerHTML=comparison;
const current=matched.samples.find(s=>s.app==='zuse');
const full=matched.fixture.legacyFullHistoryBytes;
const json=current.initialHistory.bytes;
const encoded=current.initialHistory.encodedBytes;
if(encoded){$('history-bars').innerHTML=bar('Full history',full,full,bytes,'before')+bar('Recent JSON',json,full,bytes,'before')+bar('Deflate payload',encoded,full,bytes,'');$('history-saving').textContent=(100*(1-encoded/full)).toFixed(2)+'% fewer payload bytes than full history';}
const summary=document.querySelectorAll('.summary-card')[2];
const z=matched.summary.zuse.streamWireBytes.median,t=matched.summary.t3.streamWireBytes.median;
const winner=z<t?'Zuse':'T3';
summary.innerHTML='<div class="kicker">Text stream · '+winner+' uses less data</div><div class="big '+(winner==='Zuse'?'z':'t')+'">'+bytes(Math.min(z,t))+'</div><p>Encoded socket download; median of 3 matched runs.</p>';
matchedCards();
</script>'''
html = re.sub(r'<script id="matched-chat-script">.*?</script>', '', html, flags=re.S)
html = html.replace('</body>', script + '\n</body>')
html = re.sub(r'\n{3,}', '\n\n', html)
# Retain the previous Zuse measurement so this optimization pass is reviewable.
prior_path = Path(".context/t3-benchmark/chat-before-efficiency.json")
html = re.sub(r'<section id="efficiency-pass">.*?</section>', '', html, flags=re.S)
if prior_path.exists():
    prior = json.loads(prior_path.read_text())
    cards = []
    for key, title, group, unit, note in metrics:
        if key not in ("streamWireBytes", "toolWireBytes", "olderWireBytes", "reconnectWireBytes"):
            continue
        phase = {"streamWireBytes":"stream", "toolWireBytes":"tools", "olderWireBytes":"older", "reconnectWireBytes":"reconnect"}[key]
        before = statistics.median(s["phases"][phase]["encodedWsDownloadBytes"] for s in prior["samples"] if s["app"] == "zuse")
        after = report["summary"]["zuse"][key]["median"]
        maximum = max(before, after)
        rows = ''.join(f'<div class="bar-row"><div class="bar-label">{label}</div><div class="track"><div class="fill {kind}" style="width:{value/maximum*100:.2f}%"></div></div><div class="bar-value">{value:,.0f} B</div></div>' for label,value,kind in (("Previous",before,"before"),("Current",after,"")))
        cards.append(f'<article class="chart-card"><h3>{title}</h3><p class="subtitle">Zuse before / after · medians of 3 runs</p>{rows}<p class="caption">{(1-after/before)*100:.1f}% fewer encoded download bytes</p></article>')
    section = '<section id="efficiency-pass"><div class="section-head"><h2>This efficiency pass: measured byte savings</h2></div><div class="chart-grid">' + ''.join(cards) + '</div><p class="caption">Same canonical fixture and 3G profile. These are two separate three-run batches; they do not isolate individual changes or establish confidence intervals.</p></section>'
    html = html.replace('<section id="t3-approach">', section + '\n<section id="t3-approach">', 1)
html = re.sub(r'(<b>Zuse runtime build:</b>)', r'\1 <strong>Current-main CI qualification:</strong> after the port, the existing localization and offline CJK fonts yield 8,354,245 aggregate Brotli bytes (about 167 seconds of transfer alone at 50,000 B/s). The release cap is 9 MiB. Startup charts below retain the earlier pre-localization build and are not current-main timings.<br>', html, count=1)
html = re.sub(r'\n{3,}', '\n\n', html)
html_path.write_text(html)
Path('.context/t3-benchmark/chat-summary.json').write_text(json.dumps(report["summary"], indent=2))
print(f'Updated {html_path}: {len(html.encode()):,} bytes, {len(report["samples"])} matched samples')
