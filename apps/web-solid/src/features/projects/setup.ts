// The New project flow (crates/ui/src/shell/project_setup.rs) and its folder browser
// (crates/ui/src/shell/spaces.rs AddSpaceFlow). Area-only state; the host owns the work.
import { batch } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Device, Space } from "@/core/protocol";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { navigate, setView, view } from "@/core/view";
import { noteCreation, watchCreations } from "./creations";
import { deviceList, errorText, placementComputers, spaceName, target, unknownMethod, uuid } from "./hosts";
import {
  childPath,
  cloneName,
  completionPrefixLength,
  isTypedPath,
  looksLikeGitUrl,
  matchesWords,
  parentPath,
  pathKey,
  rankFilter,
  samePath,
  segmentTarget,
  typedPathTarget,
} from "./paths";
import {
  type DriveEntry,
  type FolderEntry,
  type FolderListing,
  type GithubProjectRepositories,
  type GithubProjectRepository,
  type GithubProjectStatus,
  type HomeFolderMatch,
  type HomeFolderSearchResult,
  type Loadable,
  type ProjectCreation,
  type ProjectTemplate,
  type StartProjectCreation,
  failed,
  idle,
  loading,
  ready,
  readyValue,
} from "./types";

export type Mode = "choose" | "clone" | "create";
export type Step = "devices" | "locations" | "folders";

export const TEMPLATES: { template: ProjectTemplate; label: string; hint: string; glyph: string }[] = [
  { template: "empty", label: "Empty", hint: "A README and a Git repository", glyph: "document" },
  { template: "nextjs", label: "Next.js", hint: "TypeScript, Tailwind and the App Router", glyph: "global" },
  { template: "turborepo", label: "Turborepo", hint: "A Bun monorepo with apps and packages", glyph: "widget" },
];

interface HomeSearch {
  generation: number;
  loading: boolean;
  indexing: boolean;
  results: HomeFolderMatch[];
  started: boolean;
  unsupported: boolean;
}

export interface Browser {
  /** Open a folder as a project, or choose the parent folder for the form. */
  purpose: "open" | "parent";
  step: Step;
  deviceId: string | null;
  location: { name: string; path: string | null } | null;
  query: string;
  listing: Loadable<FolderListing>;
  drives: Loadable<DriveEntry[]>;
  browserPath: string | null;
  home: string | null;
  /** Best-effort Git seed for the open folder; the host re-verifies. */
  browserRepo: boolean;
  active: number;
  busy: boolean;
  error: string | null;
  homeSearch: HomeSearch;
  newFolder: { name: string; busy: boolean; error: string | null } | null;
}

export interface Setup {
  started: boolean;
  /** Also the operation id: repeating a submission returns the same operation. */
  generation: string;
  deviceId: string | null;
  mode: Mode;
  cloneConfirm: boolean;
  query: string;
  name: string;
  parent: string;
  url: string;
  template: ProjectTemplate;
  publish: boolean;
  github: Loadable<GithubProjectStatus>;
  repositories: Loadable<GithubProjectRepositories>;
  busy: boolean;
  error: string | null;
  active: number;
  /** A row is highlighted; until then Enter completes the step. */
  navigating: boolean;
  browser: Browser | null;
}

const blank = (): Setup => ({
  started: false,
  generation: "",
  deviceId: null,
  mode: "choose",
  cloneConfirm: false,
  query: "",
  name: "",
  parent: "",
  url: "",
  template: "empty",
  publish: false,
  github: idle(),
  repositories: idle(),
  busy: false,
  error: null,
  active: 0,
  navigating: false,
  browser: null,
});

export const [setup, setSetup] = createStore<Setup>(blank());

// ── Entry and exit ───────────────────────────────────────────────────────

let requestedDevice: string | null = null;

/** Open New project, optionally on another computer. */
export function openNewProject(deviceId?: string | null) {
  requestedDevice = deviceId ?? null;
  if (view.route.kind === "newProject" && setup.started) startSetup();
  else navigate({ kind: "newProject" });
}

export function startSetup() {
  releaseHome();
  const deviceId = requestedDevice ?? state.localDeviceId;
  requestedDevice = null;
  setSetup({ ...blank(), started: true, generation: uuid(), deviceId });
  watchCreations(deviceId);
  void loadDefaultParent();
}

/** Called when the route unmounts. */
export function endSetup() {
  releaseHome();
  setSetup(blank());
}

export function closeSetup() {
  if (setup.browser?.purpose === "parent") {
    closeBrowser();
    return;
  }
  navigate({ kind: "chat" });
}

/** Desktop's land_in_space: a new chat homed on the project. */
export function landInProject(path: string) {
  setView({ route: { kind: "chat" }, selectedChatId: null, draftProjectPath: path });
}

// ── Choose step ──────────────────────────────────────────────────────────

export type ActionRow =
  | { kind: "open" | "clone" | "create" }
  | { kind: "project"; space: Space }
  | { kind: "computer"; device: Device };

export const ACTIONS = {
  open: { glyph: "folder-open", title: "Open folder", hint: "Add an existing folder" },
  clone: { glyph: "git-branch", title: "Clone repository", hint: "From GitHub or any Git URL" },
  create: { glyph: "folder-add", title: "Create project", hint: "Start fresh from a template" },
} as const;

export function actionRows(): ActionRow[] {
  const query = setup.query;
  const rows: ActionRow[] = (["open", "clone", "create"] as const)
    .filter((kind) => matchesWords(query, `${ACTIONS[kind].title} ${ACTIONS[kind].hint}`))
    .map((kind) => ({ kind }));
  const spaces = Object.values(state.spaces)
    .filter((s) => s.deviceId === setup.deviceId && matchesWords(query, `${spaceName(s)} ${s.path}`))
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
    .slice(0, 12);
  rows.push(...spaces.map((space) => ({ kind: "project" as const, space })));
  if (!query.trim()) rows.push(...placementComputers(setup.deviceId).map((device) => ({ kind: "computer" as const, device })));
  return rows;
}

export function pickAction(row: ActionRow) {
  switch (row.kind) {
    case "project":
      landInProject(row.space.path);
      return;
    case "open":
      openBrowser("open");
      return;
    case "clone":
    case "create":
      setMode(row.kind);
      return;
    case "computer":
      pickHost(row.device.id);
  }
}

// ── Create and clone forms ───────────────────────────────────────────────

export type FormRow =
  | { kind: "template"; template: ProjectTemplate }
  | { kind: "publish" }
  | { kind: "location" }
  | { kind: "computer"; device: Device }
  | { kind: "repo"; repo: GithubProjectRepository }
  | { kind: "url"; url: string }
  | { kind: "more"; page: number };

export function formGroup(row: FormRow): string | null {
  switch (row.kind) {
    case "template":
      return "Template";
    case "publish":
      return "Options";
    case "location":
      return "Location";
    case "computer":
      return "Computer";
    case "repo":
    case "more":
      return "Your repositories";
    case "url":
      return null;
  }
}

export const searching = () => setup.mode === "clone" && !setup.cloneConfirm;

/** A URL pasted into the search wins over the picked repository. */
export function cloneUrl(): string {
  const query = setup.query.trim();
  return looksLikeGitUrl(query) ? query : setup.url.trim();
}

export function matchingRepositories(): GithubProjectRepository[] {
  const filter = setup.query.toLowerCase();
  if (looksLikeGitUrl(filter)) return [];
  const repos = readyValue(setup.repositories)?.repositories ?? [];
  return repos.filter((r) => matchesWords(filter, r.nameWithOwner));
}

export function formRows(): FormRow[] {
  if (setup.mode === "choose") return [];
  if (searching()) {
    const query = setup.query.trim();
    if (looksLikeGitUrl(query)) return [{ kind: "url", url: query }];
    const rows: FormRow[] = matchingRepositories().map((repo) => ({ kind: "repo", repo }));
    const next = readyValue(setup.repositories)?.nextPage;
    if (next) rows.push({ kind: "more", page: next });
    return rows;
  }
  const rows: FormRow[] = [];
  if (setup.mode === "create") {
    rows.push(...TEMPLATES.map(({ template }) => ({ kind: "template" as const, template })));
    rows.push({ kind: "publish" });
  }
  rows.push({ kind: "location" });
  rows.push(...placementComputers(setup.deviceId).map((device) => ({ kind: "computer" as const, device })));
  return rows;
}

export const githubReady = () => readyValue(setup.github)?.authenticated === true;

export function destination(): string | null {
  const parent = setup.parent.trim();
  const name = setup.name.trim();
  if (!parent || !name) return null;
  return childPath(parent.replace(/[\\/]+$/, "") || parent, name);
}

export function canSubmit(): boolean {
  if (setup.busy) return false;
  if (searching()) return formRows().some((row) => row.kind === "repo" || row.kind === "url");
  return destination() !== null && (setup.mode !== "clone" || cloneUrl() !== "");
}

export function setMode(mode: Mode) {
  setSetup({ mode, cloneConfirm: false, active: 0, navigating: false, error: null, query: "" });
  if (mode !== "choose") void loadGithub(1);
}

export function setQuery(query: string) {
  setSetup({ query, active: 0, navigating: false, error: null });
}

export function setName(name: string) {
  setSetup({ name, active: 0, navigating: false, error: null });
}

async function loadDefaultParent() {
  const generation = setup.generation;
  try {
    const parent = await rpc.call<string>(M.PROJECT_DEFAULT_PARENT, target(setup.deviceId));
    if (setup.generation === generation && !setup.parent.trim()) setSetup("parent", parent);
  } catch {
    if (setup.generation === generation)
      setSetup("error", "Could not load the default folder. Enter a parent folder on the selected computer.");
  }
}

export async function loadGithub(page: number) {
  const generation = setup.generation;
  const deviceId = setup.deviceId;
  if (page === 1) setSetup({ github: loading(), repositories: loading() });
  let authenticated = false;
  if (page === 1) {
    try {
      const status = await rpc.call<GithubProjectStatus>(M.PROJECT_GITHUB_STATUS, target(deviceId));
      if (setup.generation !== generation) return;
      authenticated = status.authenticated;
      setSetup({ github: ready(status) });
    } catch {
      if (setup.generation !== generation) return;
      setSetup({ github: failed("GitHub could not be reached on this computer.") });
    }
    if (!authenticated) {
      setSetup({ repositories: idle() });
      return;
    }
  }
  try {
    const result = await rpc.call<GithubProjectRepositories>(M.PROJECT_GITHUB_REPOS, target(deviceId, { page }));
    if (setup.generation !== generation) return;
    const previous = page > 1 ? readyValue(setup.repositories) : undefined;
    const repositories = previous
      ? [
          ...previous.repositories,
          ...result.repositories.filter((r) => !previous.repositories.some((p) => p.nameWithOwner === r.nameWithOwner)),
        ]
      : result.repositories;
    setSetup({ repositories: ready({ repositories, nextPage: result.nextPage }) });
  } catch {
    if (setup.generation !== generation) return;
    setSetup({ repositories: failed("Could not load repositories. Check GitHub access and retry.") });
  }
}

/** Moves the setup to another computer, keeping the step and the name. */
export function pickHost(deviceId: string) {
  if (setup.deviceId === deviceId) return;
  setSetup({ deviceId, generation: uuid(), github: idle(), repositories: idle(), error: null, parent: "" });
  watchCreations(deviceId);
  void loadDefaultParent();
  if (setup.mode !== "choose") void loadGithub(1);
}

/** From the clone destination back to the repository search, else to the actions. */
export function back() {
  if (setup.mode === "clone" && setup.cloneConfirm) {
    setSetup({ cloneConfirm: false, active: 0, navigating: false, error: null });
  } else {
    setMode("choose");
  }
}

function enterCloneConfirm() {
  setSetup({ cloneConfirm: true, active: 0, navigating: false, error: null });
}

export function chooseRepository(repo: GithubProjectRepository) {
  const name = repo.nameWithOwner.split("/").pop() || "project";
  setSetup({ url: repo.httpsUrl, name, error: null });
  enterCloneConfirm();
}

function continueWithUrl(url: string) {
  setSetup({ url, name: cloneName(url) });
  enterCloneConfirm();
}

export function activateFormRow(row: FormRow) {
  switch (row.kind) {
    case "template":
      setSetup("template", row.template);
      return;
    case "publish":
      if (githubReady()) setSetup("publish", (on) => !on);
      else if (setup.github.state !== "loading") void loadGithub(1);
      return;
    case "location":
      openBrowser("parent");
      return;
    case "computer":
      pickHost(row.device.id);
      return;
    case "repo":
      chooseRepository(row.repo);
      return;
    case "url":
      continueWithUrl(row.url);
      return;
    case "more":
      void loadGithub(row.page);
  }
}

/** Enter without a highlighted row: create, clone, or continue with the URL or first repository. */
export function primary() {
  if (searching()) {
    const first = formRows().find((row) => row.kind === "repo" || row.kind === "url");
    if (first) activateFormRow(first);
    return;
  }
  void submit();
}

export async function submit() {
  if (setup.busy || !canSubmit()) return;
  const url = cloneUrl();
  const typed = setup.name.trim();
  const clone = setup.mode === "clone";
  const request: StartProjectCreation = {
    id: setup.generation,
    name: !typed && clone ? cloneName(url) : typed,
    parent: setup.parent.trim(),
    source: clone ? { kind: "clone", url } : { kind: "create", template: setup.template, publish: setup.publish && githubReady() },
  };
  const generation = setup.generation;
  setSetup({ busy: true, error: null });
  try {
    const operation = await rpc.call<ProjectCreation>(M.PROJECT_START, target(setup.deviceId, { ...request }));
    noteCreation(operation);
    if (setup.generation === generation) landInProject(operation.destination);
  } catch (error) {
    if (setup.generation !== generation) return;
    setSetup({
      busy: false,
      error:
        error instanceof RpcError && error.outcomeUnknown
          ? "The connection dropped while starting. The project may already be setting up; check the project list before trying again."
          : errorText(error, "The project could not be started."),
    });
  }
}

// ── Folder browser ───────────────────────────────────────────────────────

const HOME_MIN_QUERY = 2;
const HOME_RESULTS = 20;
let listToken = 0;
let drivesToken = 0;

const blankHome = (unsupported = false): HomeSearch => ({
  generation: 0,
  loading: false,
  indexing: false,
  results: [],
  started: false,
  unsupported,
});

function setBrowser(update: Partial<Browser> | ((b: Browser) => void)) {
  if (!setup.browser) return;
  if (typeof update === "function") setSetup("browser", produce((b) => b && update(b)));
  else setSetup("browser", update as Browser);
}

export function openBrowser(purpose: Browser["purpose"]) {
  if (setup.busy) return;
  releaseHome();
  setSetup("browser", {
    purpose,
    step: "devices",
    deviceId: null,
    location: null,
    query: "",
    listing: idle(),
    drives: idle(),
    browserPath: null,
    home: null,
    browserRepo: false,
    active: 0,
    busy: false,
    error: null,
    homeSearch: blankHome(),
    newFolder: null,
  });
  const deviceId = setup.deviceId ?? state.localDeviceId;
  if (!deviceId) return;
  pickDevice(deviceId);
  const parent = setup.parent.trim();
  if (purpose === "parent") gotoLocation("Projects", parent || null);
  else gotoLocation("Home", null);
}

export function closeBrowser() {
  releaseHome();
  setSetup("browser", null);
}

export function pickDevice(deviceId: string) {
  setBrowser({
    step: "locations",
    location: null,
    deviceId,
    listing: idle(),
    drives: idle(),
    browserPath: null,
    home: null,
    browserRepo: false,
    active: 0,
    error: null,
    query: "",
    newFolder: null,
  });
  void loadDrives();
}

export function gotoLocation(name: string, path: string | null) {
  if (path !== null) releaseHome();
  setBrowser({ step: "folders", location: { name, path }, browserRepo: false, query: "", newFolder: null });
  void loadFolders(path);
  searchHome();
}

export function backTo(step: Step) {
  const b = setup.browser;
  if (!b) return;
  if (step === "devices" && b.purpose === "parent") {
    closeBrowser();
    return;
  }
  releaseHome();
  listToken++;
  setBrowser({
    step,
    listing: idle(),
    browserPath: null,
    location: null,
    browserRepo: false,
    active: 0,
    error: null,
    query: "",
    newFolder: null,
    ...(step === "devices" ? { deviceId: null, drives: idle(), home: null } : {}),
  });
}

async function loadDrives() {
  const b = setup.browser;
  if (!b) return;
  const token = ++drivesToken;
  setBrowser({ drives: loading() });
  try {
    const result = await rpc.call<{ drives: DriveEntry[] }>(M.LIST_DRIVES, target(b.deviceId));
    if (token === drivesToken) setBrowser({ drives: ready(result.drives ?? []) });
  } catch (error) {
    // Drives are best effort; the section keeps Home.
    if (token === drivesToken) setBrowser({ drives: failed(errorText(error)) });
  }
}

export async function loadFolders(path: string | null) {
  const b = setup.browser;
  if (!b) return;
  const token = ++listToken;
  setBrowser({ browserPath: path, listing: loading(), active: 0 });
  if (!rpc.connected) {
    setBrowser({ listing: failed("Device is not connected") });
    return;
  }
  try {
    const listing = await rpc.call<FolderListing>(M.LIST_FOLDERS, target(b.deviceId, path ? { path } : {}));
    if (token !== listToken) return;
    setBrowser((draft) => {
      if (path === null) draft.home = listing.path;
      draft.listing = ready(listing);
    });
  } catch (error) {
    if (token === listToken) setBrowser({ listing: failed(errorText(error, "Could not list folders.")) });
  }
}

export function inHome(): boolean {
  const b = setup.browser;
  return !!b && b.step === "folders" && !!b.location && b.location.path === null;
}

export function browserDevices(): Device[] {
  const b = setup.browser;
  return b ? rankFilter(b.query, deviceList(), (d) => d.name) : [];
}

export function browserLocations(): { name: string; path: string | null }[] {
  const b = setup.browser;
  if (!b) return [];
  const drives = readyValue(b.drives) ?? [];
  const locations = [{ name: "Home", path: null as string | null }, ...drives.map((d) => ({ name: d.name, path: d.path as string | null }))];
  return rankFilter(b.query, locations, (l) => l.name);
}

function filteredFolders(): FolderEntry[] {
  const b = setup.browser;
  const listing = b && b.step === "folders" ? readyValue(b.listing) : undefined;
  if (!b || !listing) return [];
  return rankFilter(
    b.query,
    listing.entries.filter((e) => e.isDir),
    (e) => e.name,
  );
}

export type FolderRow = { kind: "local"; entry: FolderEntry; path: string } | { kind: "home"; match: HomeFolderMatch };

/** The open folder's matching subfolders, then matches from anywhere under home not already listed. */
export function folderRows(): FolderRow[] {
  const b = setup.browser;
  if (!b) return [];
  const listing = readyValue(b.listing);
  const local = filteredFolders().map((entry) => ({ kind: "local" as const, entry, path: listing ? childPath(listing.path, entry.name) : entry.name }));
  if (!inHome()) return local;
  const listed = new Set([...local.map((row) => pathKey(row.path)), ...(listing ? [pathKey(listing.path)] : [])]);
  const home = b.homeSearch.results.filter((m) => !listed.has(pathKey(m.path))).map((match) => ({ kind: "home" as const, match }));
  return [...local, ...home];
}

export function browserRowCount(): number {
  const b = setup.browser;
  if (!b) return 0;
  if (b.step === "devices") return browserDevices().length;
  if (b.step === "locations") return browserLocations().length;
  return folderRows().length;
}

export function setBrowserActive(active: number) {
  setBrowser({ active });
}

export function setBrowserQuery(query: string) {
  setBrowser({ query });
  if (slashDescend()) return;
  setBrowser({ active: 0 });
  searchHome();
}

export function descend(path: string, isRepo: boolean) {
  setBrowser({ browserRepo: isRepo, query: "", newFolder: null });
  void loadFolders(path);
}

export function openActive() {
  const b = setup.browser;
  if (!b) return;
  if (b.step === "devices") {
    const device = browserDevices()[b.active];
    if (device) pickDevice(device.id);
    return;
  }
  if (b.step === "locations") {
    const location = browserLocations()[b.active];
    if (location) gotoLocation(location.name, location.path);
    return;
  }
  const rows = folderRows();
  if (!rows.length) {
    const path = typedPathTarget(b.query, b.home);
    if (path) descend(path, false);
    return;
  }
  const row = rows[b.active];
  if (!row) return;
  if (row.kind === "home") descend(row.match.path, row.match.isRepo);
  else descend(row.path, row.entry.isRepo);
}

/** `name/` descends into a folder of the listing; a typed path with a trailing separator browses it. */
function slashDescend(): boolean {
  const b = setup.browser;
  if (!b || b.step !== "folders") return false;
  const text = b.query;
  if (isTypedPath(text) && /[\\/]$/.test(text)) {
    const path = typedPathTarget(text, b.home);
    if (!path) return false;
    descend(path, false);
    return true;
  }
  if (!text.endsWith("/")) return false;
  const name = text.slice(0, -1);
  if (!name || name.includes("/")) return false;
  const listing = readyValue(b.listing);
  if (!listing) return false;
  const dirs = listing.entries.filter((e) => e.isDir);
  const at = segmentTarget(
    dirs.map((d) => d.name),
    name,
  );
  if (at === null) return false;
  const entry = dirs[at]!;
  descend(childPath(listing.path, entry.name), entry.isRepo);
  return true;
}

/** The faint suffix Tab accepts: the highlighted folder when the query prefixes it, else the first prefix match. */
export function completion(): { name: string; suffix: string } | null {
  const b = setup.browser;
  if (!b || b.step !== "folders" || !b.query) return null;
  const rows = filteredFolders();
  const fits = (e: FolderEntry) => completionPrefixLength(e.name, b.query) !== null;
  const highlighted = rows[b.active];
  const entry = highlighted && fits(highlighted) ? highlighted : rows.find(fits);
  if (!entry || b.query.length >= entry.name.length) return null;
  return { name: entry.name, suffix: entry.name.slice(b.query.length) };
}

export function acceptCompletion() {
  const next = completion();
  if (next) setBrowser({ query: next.name });
}

export function goUp() {
  const b = setup.browser;
  if (!b) return;
  if (b.step === "devices") return;
  if (b.step === "locations") {
    backTo("devices");
    return;
  }
  const listing = readyValue(b.listing);
  const root = b.location?.path ?? b.home;
  const parent = listing && !(root && samePath(listing.path, root)) ? parentPath(listing.path) : null;
  if (parent) descend(parent, false);
  else backTo("locations");
}

/** Add the open folder (not the highlight), or hand it back to the form as the parent. */
export async function submitBrowser() {
  const b = setup.browser;
  if (!b || b.busy || b.step !== "folders" || !b.deviceId) return;
  const listing = readyValue(b.listing);
  if (!listing) return;
  const path = listing.path;
  if (b.purpose === "parent") {
    setSetup({ parent: path, error: null });
    closeBrowser();
    return;
  }
  const deviceId = b.deviceId;
  // The engine dedupes (device, folder) pairs, so creating would leave a dangling id.
  const existing = Object.values(state.spaces).find((s) => s.deviceId === deviceId && samePath(s.path, path));
  if (existing) {
    landInProject(existing.path);
    return;
  }
  setBrowser({ busy: true, error: null });
  try {
    await rpc.call(M.MUTATE, { op: "createSpace", spaceId: uuid(), deviceId, path, gitDetected: b.browserRepo });
    landInProject(path);
  } catch (error) {
    setBrowser({ busy: false, error: errorText(error, "The folder could not be added.") });
  }
}

export function startNewFolder() {
  const b = setup.browser;
  if (!b || b.step !== "folders" || b.listing.state !== "ready") return;
  setBrowser({ newFolder: { name: "", busy: false, error: null } });
}

export function setNewFolderName(name: string) {
  setBrowser((draft) => {
    if (draft.newFolder) {
      draft.newFolder.name = name;
      draft.newFolder.error = null;
    }
  });
}

export function cancelNewFolder() {
  setBrowser({ newFolder: null });
}

export async function createFolder() {
  const b = setup.browser;
  const listing = b ? readyValue(b.listing) : undefined;
  const name = b?.newFolder?.name.trim() ?? "";
  if (!b || !listing || !b.newFolder || b.newFolder.busy) return;
  if (!name || /[\\/]/.test(name) || name === "." || name === "..") {
    setBrowser((draft) => draft.newFolder && (draft.newFolder.error = "Enter a folder name without slashes."));
    return;
  }
  setBrowser((draft) => draft.newFolder && (draft.newFolder.busy = true));
  try {
    const result = await rpc.call<{ path: string }>(M.CREATE_FOLDER, target(b.deviceId, { parent: listing.path, name }));
    if (setup.browser?.newFolder) descend(result.path, false);
  } catch (error) {
    setBrowser((draft) => {
      if (!draft.newFolder) return;
      draft.newFolder.busy = false;
      draft.newFolder.error = errorText(error, "The folder could not be created.");
    });
  }
}

// ── Home search ──────────────────────────────────────────────────────────

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Look the query up under the host's home. The first call in Home starts the host's index. */
export function searchHome() {
  const b = setup.browser;
  if (!b) return;
  const query = b.query.trim();
  const generation = b.homeSearch.generation + 1;
  setBrowser((draft) => {
    draft.homeSearch.generation = generation;
    draft.homeSearch.loading = false;
    draft.homeSearch.indexing = false;
    draft.homeSearch.results = [];
  });
  if (!inHome() || b.homeSearch.unsupported || !rpc.connected) return;
  if (!b.homeSearch.started) {
    setBrowser((draft) => (draft.homeSearch.started = true));
    void requestHome(b.deviceId, "", null);
  }
  if (query.length < HOME_MIN_QUERY || isTypedPath(query)) return;
  setBrowser((draft) => (draft.homeSearch.loading = true));
  void requestHome(b.deviceId, query, generation);
}

/** Refresh until the host finishes indexing, backing off between replies. `null` only warms the index. */
async function requestHome(deviceId: string | null, query: string, generation: number | null) {
  let delay = 500;
  for (;;) {
    let result: HomeFolderSearchResult | null = null;
    let unsupported = false;
    try {
      result = await rpc.call<HomeFolderSearchResult>(M.SEARCH_HOME_FOLDERS, target(deviceId, { query, limit: HOME_RESULTS }));
    } catch (error) {
      unsupported = unknownMethod(error);
    }
    const b = setup.browser;
    if (!b || b.deviceId !== deviceId) return;
    if (generation !== null && generation !== b.homeSearch.generation) return;
    if (unsupported) {
      setBrowser((draft) => (draft.homeSearch = { ...blankHome(true) }));
      return;
    }
    if (generation === null || b.homeSearch.unsupported) return;
    const selected = folderRows()[b.active];
    batch(() => {
      setBrowser((draft) => {
        draft.homeSearch.loading = false;
        draft.homeSearch.indexing = result?.indexing ?? false;
        if (result) draft.homeSearch.results = result.matches;
      });
      // New partial results may reorder the rows under the cursor.
      if (selected) {
        const key = selected.kind === "home" ? selected.match.path : selected.path;
        const at = folderRows().findIndex((row) => (row.kind === "home" ? row.match.path : row.path) === key);
        if (at >= 0) setBrowser({ active: at });
      }
    });
    if (!result?.indexing) return;
    await sleep(delay);
    delay = Math.min(delay * 2, 2000);
    if (!inHome() || setup.browser?.homeSearch.generation !== generation) return;
  }
}

/** The picker is done with the host's home index (it stays when a projectless chat warmed it). */
export function releaseHome() {
  const b = setup.browser;
  if (!b || !b.homeSearch.started) return;
  const deviceId = b.deviceId;
  setBrowser((draft) => (draft.homeSearch = blankHome(draft.homeSearch.unsupported)));
  if (rpc.connected) void rpc.call(M.RELEASE_HOME_FOLDER_SEARCH, target(deviceId)).catch(() => {});
}
