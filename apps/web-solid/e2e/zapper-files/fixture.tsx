// Zapper Files rendered from the real components with an in-memory file store. No engine, no paid services.
import type { Zapper, ZapperAction, ZapperFile } from "@/core/zapper-types";
import { setInspectorOpen } from "@/features/zappers/state";
import { App } from "@/shell/App";
import { setView } from "@/core/view";
import "@/shell/shell.css";
import "@/shell/theme";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { setHostedBrowser } from "@/core/auth";
import { setState } from "@/core/store";
import { rpc } from "@/core/connection";
import "@/ui/tokens.css";
import "@/ui/primitives.css";
import "@/features/workspace/workspace.css";

const reserved = (rules = "") => [
  { path: "AGENTS.md", version: rules ? 2 : 1, content: rules },
  { path: "DIGEST.md", version: 3, content: "# Digest\n\n- Release branch is green." },
  { path: "INSTRUCTIONS.md", version: 2, content: "I keep the release train moving and report what needs a decision." },
  { path: "NOTES.md", version: 4, content: "# Notes\n\n- Design review moved to Thursday." },
  { path: "WORKERS.md", version: 2, content: "- Run the full test suite before reporting.\n- Never merge." },
];
const nested: ZapperFile[] = [
  ...reserved("# Rules\n\n- Ask before opening pull requests that touch billing.\n- Keep threads to one issue each."),
  { path: "handoffs/conversational-pr-research.md", version: 1, content: "# Research\n\nNotes from the conversational review study." },
  { path: "handoffs/design-references/composer.md", version: 2, content: "# Composer\n\nSpacing and states." },
  { path: "handoffs/design-references/sidebar.md", version: 1, content: "# Sidebar" },
  { path: "reports/2026-11-26.md", version: 1, content: "# Weekly report" },
  { path: "reports/old.md", version: 1, content: "old" },
  { path: "reports/old.md", version: 2, content: "", deleted: true },
];
let files: ZapperFile[] = structuredClone(nested);
let fail: string | null = null;
let listError: string | null = null;
const zapper: Zapper = {
  id: "release", revision: 1, activityRevision: 0, status: "active", coordinatorChatId: "coordinator", previousCoordinatorChats: [],
  config: { name: "Release", purpose: "Keep the release train moving.", homeDeviceId: "local", workerDeviceId: "local", repositories: [], coordinatorModel: { harness: "claude-code", model: null }, maxConcurrentWorkers: 3, maxFollowups: 2 },
  workers: [], decisions: [], issues: [], folders: ["handoffs", "handoffs/design-references", "reports"],
};
const live = () => {
  const latest = new Map<string, ZapperFile>();
  for (const f of files) if ((latest.get(f.path)?.version ?? 0) < f.version) latest.set(f.path, f);
  return latest;
};
function apply(action: ZapperAction) {
  const latest = live();
  const add = (path: string, content: string, deleted = false) => files.push({ path, version: (latest.get(path)?.version ?? 0) + 1, content, ...(deleted ? { deleted } : {}) });
  if (action.type === "writeFile") add(action.path, action.content);
  if (action.type === "deleteFile") add(action.path, "", true);
  if (action.type === "moveFile") { add(action.to, latest.get(action.path)!.content); add(action.path, "", true); }
  if (action.type === "createFolder") zapper.folders = [...(zapper.folders ?? []), action.path].sort();
  if (action.type === "deleteFolder") {
    for (const f of action.files) add(f.path, "", true);
    zapper.folders = (zapper.folders ?? []).filter((p) => p !== action.path && !p.startsWith(action.path + "/"));
  }
  if (action.type === "moveFolder") {
    for (const f of action.files) { add(action.to + f.path.slice(action.path.length), latest.get(f.path)!.content); add(f.path, "", true); }
    zapper.folders = (zapper.folders ?? []).map((p) => (p === action.path || p.startsWith(action.path + "/") ? action.to + p.slice(action.path.length) : p)).sort();
  }
}
const requests: { method: string; params: unknown }[] = [];
Object.assign(rpc, {
  call: async (method: string, params: unknown) => {
    requests.push({ method, params });
    if (method === "ZotEvents") return [];
    if (method === "ZotFiles") { if (listError) throw new Error(listError); return structuredClone(files); }
    if (method === "ZotCommand") {
      if (fail) throw new Error(fail);
      apply((params as { action: ZapperAction }).action);
      zapper.revision++;
      return structuredClone(zapper);
    }
    throw new Error("Fixture has no services");
  },
  subscribe: (method: string, params: unknown) => { requests.push({ method, params }); return { cancel: () => {} }; },
});
const show = () => {
  setState((draft) => { draft.zappers = [structuredClone(zapper)]; draft.epoch++; });
  flush();
};
Object.assign(window, {
  files: {
    open: () => {
      setHostedBrowser(false);
      setState((draft) => {
        draft.connection = { kind: "connected" };
        draft.account = { user: null };
        draft.workspaceSynced = true; draft.localDeviceId = "local"; draft.zappersReady = true; draft.zappersSupported = true;
        draft.chats.coordinator = { id: "coordinator", deviceId: "local", archived: false, createdAt: "", title: "Release", config: { harness: "claude-code", sandbox: "auto" } };
        draft.zappers = [structuredClone(zapper)];
      });
      setView((draft) => { draft.route = { kind: "zappers", zapperId: "release" }; draft.selectedChatId = "coordinator"; draft.rightPanel = null; });
      setInspectorOpen(true); flush();
    },
    empty: () => { files = reserved(); zapper.folders = []; show(); },
    nested: () => { files = structuredClone(nested); zapper.folders = ["handoffs", "handoffs/design-references", "reports"]; show(); },
    fail: (message: string | null) => { fail = message; },
    listError: (message: string | null) => { listError = message; show(); },
    change: (path: string, content: string) => { files.push({ path, version: (live().get(path)?.version ?? 0) + 1, content }); },
    theme: (theme: "light" | "dark") => { setView((draft) => { draft.preferences.theme = theme; }); flush(); },
    requests: () => requests,
    files: () => [...live().values()].filter((f) => !f.deleted).map((f) => f.path).sort(),
    folders: () => zapper.folders,
  },
});
render(() => <App />, document.getElementById("root")!);
