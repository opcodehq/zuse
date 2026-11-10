// Sidebar view options, persisted with the rest of view.preferences.
import { setView, view } from "@/core/view";

export interface SidebarOptions {
  sort: "updated" | "created";
  compact: boolean;
  showBranch: boolean;
  showHarness: boolean;
  showProjectIcon: boolean;
  showLocation: boolean;
  showProjectFilter: boolean;
  byDevice: boolean;
  pinnedOpen: boolean;
  spaceFilter: string | null;
}

// Desktop's defaults (crates/ui settings), minus the pull request line this client does not watch.
const defaults: SidebarOptions = {
  sort: "updated",
  compact: true,
  showBranch: true,
  showHarness: true,
  showProjectIcon: true,
  showLocation: true,
  showProjectFilter: true,
  byDevice: false,
  pinnedOpen: true,
  spaceFilter: null,
};

const keyFor = (key: keyof SidebarOptions) => `sidebar.${key}`;

export function option<K extends keyof SidebarOptions>(key: K): SidebarOptions[K] {
  const value = view.preferences[keyFor(key)];
  return (value === undefined ? defaults[key] : value) as SidebarOptions[K];
}

export function setOption<K extends keyof SidebarOptions>(key: K, value: SidebarOptions[K]) {
  (setView as (...args: unknown[]) => void)("preferences", keyFor(key), value);
}

export type Organization = "projects" | "threads" | "devices";

export const organization = (): Organization =>
  option("byDevice") ? "devices" : view.preferences.sidebarView === "threads" ? "threads" : "projects";

export function setOrganization(next: Organization) {
  if (next === "devices") setOption("byDevice", true);
  else {
    setOption("byDevice", false);
    setView("preferences", "sidebarView", next);
  }
}

export function isCollapsed(key: string) {
  return view.preferences.collapsedProjects.includes(key);
}

export function toggleCollapsed(key: string) {
  const list = view.preferences.collapsedProjects;
  setView("preferences", "collapsedProjects", list.includes(key) ? list.filter((k) => k !== key) : [...list, key]);
}
