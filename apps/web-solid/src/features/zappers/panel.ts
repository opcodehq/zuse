// Mirrors protocol::presentation::ZapperPanelTabs: sections, closable pages and
// a navigation trail, independent for each Zapper.
import { createStore } from "solid-js";
export type Section = "overview" | "issues" | "threads" | "drive";
export type PanelPage = { kind: "section"; name: Section } | { kind: "issue"; id: string } | { kind: "file"; path: string } | { kind: "history" } | { kind:"agent"; docId:string; chatId:string; title?:string } | { kind:"diff"; chatId:string } | {kind:"pr";url:string;chatId:string};
interface Panel { active: PanelPage; open: PanelPage[]; trail: PanelPage[] }
const [panels, setPanels] = createStore<Record<string, Panel>>({});
const empty: Panel = {active:{kind:"section",name:"overview"},open:[],trail:[]};
export const pageKey = (page: PanelPage) => page.kind === "section" ? `section:${page.name}` : page.kind === "issue" ? `issue:${page.id}` : page.kind === "file" ? `file:${page.path}` : page.kind === "agent" ? `agent:${page.docId}` : page.kind === "diff" ? `diff:${page.chatId}` : page.kind === "pr" ? `pr:${page.url}` : "history";
export const panelFor = (id: string) => panels[id] ?? empty;
export function showPage(id: string, page: PanelPage) {
  setPanels(draft => {
    if(!draft[id]) draft[id]={active:{kind:"section",name:"overview"},open:[],trail:[]};
    const panel = draft[id]!;
    if (pageKey(panel.active) === pageKey(page)) return;
    panel.trail = [...panel.trail.filter(item=>pageKey(item)!==pageKey(page)),panel.active].slice(-64);
    if (page.kind !== "section" && !panel.open.some(item=>pageKey(item)===pageKey(page))) panel.open.push(page);
    panel.active = page;
  });
}
export function closePage(id: string, page: PanelPage) {
  setPanels(draft => {
    const panel = draft[id]; if (!panel) return;
    const key = pageKey(page);
    panel.open = panel.open.filter(item=>pageKey(item)!==key);
    panel.trail = panel.trail.filter(item=>pageKey(item)!==key);
    if (pageKey(panel.active) === key) panel.active = panel.trail.pop() ?? {kind:"section",name:"overview"};
  });
}
