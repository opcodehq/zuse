// Local view state: what is open, not what exists. Persisted preferences live here too.
import { createEffect, createRoot, createSignal, on } from "solid-js";
import { createStore } from "solid-js/store";

export type Route =
  | { kind: "chat" }
  | { kind: "newProject" }
  | { kind: "settings"; section?: string }
  | { kind: "plugins" }
  | { kind: "usage" }
  | { kind: "archive"; projectKey?: string };

export type RightPanel = "files" | "changes" | "terminal" | "browser" | "git" | "history" | "pullRequest" | null;

export interface Preferences {
  theme: "system" | "dark" | "light";
  sidebarView: "projects" | "threads";
  sidebarWidth: number;
  sidebarOpen: boolean;
  collapsedProjects: string[];
  rightPanelWidth: number;
  language: string;
  /** New-chat defaults (Settings → Default models). Null model follows the composer's own choice. */
  defaultPermissionMode: "approval-required" | "auto" | "full-access";
  defaultModel: { provider: string; model: string; label: string } | null;
  [key: string]: unknown;
}

const PREFERENCES = "zuse.web.preferences.v1";
const defaults: Preferences = {
  theme: "system",
  sidebarView: "projects",
  sidebarWidth: 232,
  sidebarOpen: true,
  collapsedProjects: [],
  rightPanelWidth: 420,
  language: "system",
  defaultPermissionMode: "approval-required",
  defaultModel: null,
};

function load(): Preferences {
  try {
    const raw = localStorage.getItem(PREFERENCES);
    return raw ? { ...defaults, ...(JSON.parse(raw) as Partial<Preferences>) } : defaults;
  } catch {
    return defaults;
  }
}

export interface ViewState {
  route: Route;
  selectedChatId: string | null;
  /** The project a new chat will start in, when none is selected. */
  draftProjectPath: string | null;
  rightPanel: RightPanel;
  preferences: Preferences;
  /** The sidebar drawer in the narrow layout; never persisted, so a phone always opens on the chat. */
  drawerOpen: boolean;
}

// What this tab had open survives a reload, as desktop reopens the last chat. Per tab, so two tabs can differ.
const TAB = "zuse.web.tab.v1";
type TabState = Pick<ViewState, "selectedChatId" | "draftProjectPath" | "rightPanel">;
function loadTab(): TabState {
  const empty: TabState = { selectedChatId: null, draftProjectPath: null, rightPanel: null };
  try {
    const raw = sessionStorage.getItem(TAB);
    if (!raw) return empty;
    const value = JSON.parse(raw) as Partial<TabState>;
    return {
      selectedChatId: typeof value.selectedChatId === "string" ? value.selectedChatId : null,
      draftProjectPath: typeof value.draftProjectPath === "string" ? value.draftProjectPath : null,
      rightPanel: typeof value.rightPanel === "string" ? value.rightPanel : null,
    };
  } catch {
    return empty;
  }
}
const tab = loadTab();
/** The chat restored from the last session of this tab, until the first chat list confirms it still exists. */
export const restoredChatId = tab.selectedChatId;

export const [view, setView] = createStore<ViewState>({
  route: { kind: "chat" },
  ...tab,
  preferences: load(),
  drawerOpen: false,
});

/** Phones and narrow windows (the old client switched at the same width): the sidebar becomes a drawer. */
const NARROW = "(max-width: 800px)";
export const narrowLayout = createRoot(() => {
  if (typeof matchMedia !== "function") return () => false;
  const query = matchMedia(NARROW);
  const [narrow, setNarrow] = createSignal(query.matches);
  query.addEventListener("change", (event) => setNarrow(event.matches));
  return narrow;
});

/** Whether the sidebar shows: the persisted preference on wide screens, the drawer on narrow ones. */
export const sidebarVisible = () => (narrowLayout() ? view.drawerOpen : view.preferences.sidebarOpen);
export function setSidebarVisible(open: boolean) {
  if (narrowLayout()) setView("drawerOpen", open);
  else setView("preferences", "sidebarOpen", open);
}

createRoot(() => {
  // Picking a chat, a project or another screen from the drawer closes it.
  createEffect(on(() => [view.selectedChatId, view.draftProjectPath, view.route] as const, () => setView("drawerOpen", false), { defer: true }));
  createEffect(on(narrowLayout, () => setView("drawerOpen", false), { defer: true }));
  createEffect(() => {
    const text = JSON.stringify(view.preferences);
    try {
      localStorage.setItem(PREFERENCES, text);
    } catch {
      // Storage can be unavailable in private windows; preferences then last for the tab.
    }
  });
  createEffect(() => {
    const text = JSON.stringify({ selectedChatId: view.selectedChatId, draftProjectPath: view.draftProjectPath, rightPanel: view.rightPanel });
    try {
      sessionStorage.setItem(TAB, text);
    } catch {
      // As above.
    }
  });
});

export function openChat(chatId: string | null) {
  setView({ selectedChatId: chatId, route: { kind: "chat" } });
}
export function navigate(route: Route) {
  setView("route", route);
}
