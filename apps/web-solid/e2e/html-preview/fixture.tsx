import { render } from "@solidjs/web";
import { createSignal, flush, Show } from "solid-js";
import { paintDitherAreas } from "@/ui/dither-chart";
import { Markdown } from "@/features/transcript/Markdown";
import { ReferenceMarkdown } from "@/features/transcript/ReferenceMarkdown";
import { referenceCatalog } from "@/features/transcript/references";
import { setView } from "@/core/view";
import { previewDocument, hasClosedFence } from "@/features/transcript/html-preview";
import "@/ui/tokens.css";
import "@/features/transcript/transcript.css";

const sample = `<h1>One machine. Many worlds.</h1><p>A small interactive model, right inside the conversation.</p><div style="display:grid;grid-template-columns:1fr 1fr;gap:12px;margin:20px 0"><div style="padding:18px;border:1px solid var(--border);border-radius:10px"><b>Guest A</b><p>Private memory · own kernel</p></div><div style="padding:18px;border:1px solid var(--border);border-radius:10px"><b>Guest B</b><p>Private memory · own kernel</p></div></div><button id="counter">Run a workload · 0</button><script>let count=0;document.querySelector('#counter').addEventListener('click',event=>{event.target.textContent='Run a workload · '+(++count)});window.loadId=Math.random();</script>`;
const fenced = (code: string) => '```html\n' + code + '\n```';
function Fixture() {
  const [text, setText] = createSignal('```html\n' + sample);
  const [live, setLive] = createSignal(true);
  const [previews, setPreviews] = createSignal(true);
  const [references, setReferences] = createSignal(false);
  const catalog = referenceCatalog({ issues: [], pullRequests: [], threads: [], agents: [] });
  Object.assign(window, { previewFixture: {
    useReferences: () => flush(() => setReferences(true)),
    disablePreviews: () => flush(() => setPreviews(false)),
    complete: () => flush(() => { setText(fenced(sample)); setLive(false); }),
    replace: (code: string) => flush(() => { setText(fenced(code)); setLive(false); }),
    theme: (theme: "dark" | "light") => flush(() => setView(draft => { draft.preferences.theme = theme; })),
    plain: () => flush(() => setText('```js\nconst x = 1;\n```\n\n<div id="raw-injection">raw HTML</div>')),
    build: previewDocument,
    hasClosedFence,
    chartReference: () => {
      const canvas = document.createElement('canvas'); canvas.width = 64; canvas.height = 32;
      const ctx = canvas.getContext('2d')!;
      paintDitherAreas(ctx, 64, 32, 100, [[30], [80], [50]], [[53, 143, 243]]);
      return Array.from(ctx.getImageData(0, 0, 64, 32).data);
    },
  } });
  return <div class="t-root" style={{ "max-width": "900px", margin: "40px auto", padding: "20px" }}><div style={{ "font-size": "12px", color: "var(--text-muted)", "margin-bottom": "24px" }}>ZUSE / CONVERSATION</div><Show when={references()} fallback={<Markdown text={text()} live={live()} htmlPreview={previews()} />}><ReferenceMarkdown text={text()} live={live()} htmlPreview={previews()} chatId="fixture" references={catalog} /></Show></div>;
}
render(() => <Fixture />, document.getElementById("root")!);
