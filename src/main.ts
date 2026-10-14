import { api, type Job, type Settings, type View } from "./api";
import { diffId, isDiff, keep, owner as ownerOf } from "./diff";
import { escapePath, pathsFromUris, safeName } from "./drop";
import { move, neighbor, panes as paneIds, remove, setRatio, split, swap, type Node, type Side, type Tree } from "./layout";
import * as panes from "./panes";
import { openPicker } from "./picker";
import * as sidebar from "./sidebar";
import { moveSpace, place, without } from "./spaces";
import * as themes from "./themes";
import { openPrefs, type Section } from "./prefs";
import { matches, type Chord, type Command } from "./shortcuts";
import * as terms from "./terminals";
import { openFind } from "./findbar";
import { openPalette, type Item } from "./palette";
import { initTips } from "./tip";

const list = document.getElementById("list")!;
const host = document.getElementById("terms")!;
const empty = document.getElementById("empty")!;

let views: View[] = [];
let jobs: Job[] = []; // Claude's background jobs no pane shows
let spaces: Node[] = []; // workspaces, in sidebar order
let tree: Tree = null; // the workspace on screen: always one of `spaces`, or null when there are none
let focusId: string | null = null;
let zoomed: string | null = null; // a pane filling the window; its workspace's layout is kept
let recentRepos: string[] = [];
let claudeFound = true;
let shell = "zsh";
let settings: Settings = { newSessionFolder: null, quitStopsSessions: false, notifyNeedsYou: true, notifyDone: true, claudeMatchesTheme: true };
const unread = new Set<string>();

const byId = (id: string | null) => views.find((v) => v.id === id);
const parentOf = (id: string) => byId(id)?.parent;
/** The session a pane belongs to: itself, or the session of a diff pane. */
const owner = (id: string | null) => (id ? ownerOf(id) : "");
const looking = (id: string) => document.hasFocus() && owner(focusId) === id;

/** Finished or waiting while you weren't looking: marked until you focus it. */
function track(prev: View | undefined, v: View) {
  if (!prev || prev.status === v.status) return;
  const finished = v.status === "idle" && prev.status === "working";
  if ((finished || v.status === "needs_you") && !looking(v.id)) unread.add(v.id);
}

function markRead(id: string | null) {
  if (id && document.hasFocus() && unread.delete(owner(id))) drawSidebar();
}

function setRail(on: boolean) {
  document.body.classList.toggle("rail", on);
  try {
    localStorage.setItem("roost.rail", on ? "1" : "0");
  } catch {
    // no storage: the rail just isn't remembered
  }
}
const ARROWS: Record<string, Side> = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "top", ArrowDown: "bottom" };

function report(err: unknown) {
  console.error(err);
  const t = document.createElement("div");
  t.className = "toast";
  t.textContent = String(err);
  document.body.append(t);
  setTimeout(() => t.remove(), 4000);
}

let saveTimer: number | undefined;
function persist() {
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => void api.saveLayout({ spaces }, focusId).catch(report), 300);
}

const hooks: panes.Hooks = {
  focus: (id) => focusPane(id),
  rename: (id, nameEl) => renameRow(id, nameEl),
  drop: (id, target, zone) => {
    spaces = without(spaces, id, tree); // dragged in from another workspace: it leaves that one
    const before = paneIds(tree);
    setTree(move(tree, id, target, zone), id);
    // dropped on a pane's middle, it takes that pane's place: the one it replaced gets a workspace below
    const displaced = before.filter((p) => p !== id && !isDiff(p) && !paneIds(tree).includes(p));
    if (displaced.length) {
      spaces.splice(spaces.indexOf(tree!) + 1, 0, ...displaced.map((p) => ({ pane: p })));
      drawSidebar();
      persist();
    }
    if (!isDiff(id)) void api.select(id).catch(report);
  },
  diff: (id) => toggleDiff(id),
  files: (id, files, uriList) => void dropFiles(id, files, uriList),
  fork: (id) => void forkSession(id),
  ratio: (path, r) => setTree(setRatio(tree, path, r), focusId),
};

/** Each workspace's sessions, in pane order (diff panes stand for nothing). */
const groups = () => spaces.map((t) => paneIds(t).filter((id) => !isDiff(id)));
const spaceOf = (id: string) => spaces.findIndex((t) => paneIds(t).includes(id));

function drawSidebar() {
  if (list.querySelector(".rename")) return; // never wipe a rename in progress
  sidebar.render(list, views, groups(), spaces.indexOf(tree!), owner(focusId), unread, jobs, {
    openJob: (id) => void openJob(id),
    select: (id) => void show(id),
    rename: (id, name) => void api.rename(id, name).catch(report),
    popOut: (id, target, after) => popOut(id, target, after),
    swap: (id, target) => swapPanes(id, target),
    reorder: (id, target, after) => {
      spaces = moveSpace(spaces, spaceOf(id), spaceOf(target), after);
      drawSidebar();
      persist();
    },
  });
}

function drawEmpty() {
  empty.hidden = tree !== null;
  empty.textContent = claudeFound
    ? "Press ⌘N to start a session."
    : "Claude Code not found: terminals work, `claude` won't. Install it, or set ROOST_CLAUDE_BIN.";
}

/** Every change to the workspace on screen goes through here; emptied, it goes and the one above shows. */
function setTree(t: Tree, focus: string | null) {
  const i = spaces.indexOf(tree!);
  if (!t) {
    if (i >= 0) spaces.splice(i, 1);
    return showSpace(Math.max(0, i - 1), null);
  }
  if (i >= 0) spaces[i] = t;
  else spaces.push(t);
  tree = t;
  draw(focus);
}

/** Puts a workspace on screen as it was arranged, waking its sessions. */
function showSpace(i: number, focus: string | null) {
  tree = spaces[i] ?? null;
  draw(focus);
  for (const id of paneIds(tree)) if (!isDiff(id)) void api.select(id).catch(report);
}

/** Render the workspace on screen, redraw the sidebar, focus, save. */
function draw(focus: string | null) {
  const ids = paneIds(tree);
  focusId = focus && ids.includes(focus) ? focus : (ids[0] ?? null);
  if (zoomed !== focusId) zoomed = null; // another pane (or workspace) took the focus
  host.classList.toggle("zoomed", zoomed !== null);
  panes.render(host, zoomed ? { pane: zoomed } : tree, focusId, views, shell, hooks);
  drawEmpty();
  drawSidebar();
  if (focusId) terms.focus(focusId);
  persist();
  markRead(focusId);
}

function focusPane(id: string) {
  if (focusId === id) return;
  if (zoomed) return draw(id); // the zoomed pane makes way for the whole workspace
  focusId = id;
  panes.setFocus(id);
  terms.focus(id);
  drawSidebar();
  persist();
  markRead(id);
}

/** Focus the session's pane, switching to its workspace (one of its own if it has none). */
async function show(id: string) {
  if (paneIds(tree).includes(id)) focusPane(id);
  else {
    if (spaceOf(id) < 0) spaces.push({ pane: id });
    showSpace(spaceOf(id), id);
  }
  try {
    await api.select(id);
  } catch (e) {
    report(e);
  }
}

function defaultRepo(): string | undefined {
  return byId(owner(focusId))?.repo ?? recentRepos[0];
}

/** A plain terminal, at once: in the focused session's folder, else the last one, else home. */
async function newSession() {
  try {
    const v = await api.create("", settings.newSessionFolder ?? defaultRepo() ?? "", false);
    await show(v.id);
  } catch (e) {
    report(e);
  }
}

async function resumePast() {
  const repo = defaultRepo() ?? (await api.pickFolder());
  if (!repo) return;
  void openPicker({
    repo,
    pick: async (p) => {
      try {
        const v = await api.resumePast(repo, p);
        await show(v.id);
        return true;
      } catch (e) {
        report(e);
        return false;
      }
    },
    closed: () => focusId && terms.focus(focusId),
  });
}

async function splitFocused(side: Side) {
  const src = byId(owner(focusId));
  if (!src || !focusId) return void newSession();
  try {
    const v = await api.create("", src.repo, false);
    spaces = without(spaces, v.id, tree); // its sessions_changed may have given it a workspace already
    setTree(split(tree, focusId, v.id, side), v.id);
  } catch (e) {
    report(e);
  }
}

function neighborOf(id: string): string | null {
  const r = panes.rects();
  for (const d of ["right", "left", "bottom", "top"] as Side[]) {
    const n = neighbor(r, id, d);
    if (n) return n;
  }
  return null;
}

async function closeFocused() {
  if (focusId && isDiff(focusId)) return removePane(); // closing a diff never closes its session
  const v = byId(focusId);
  if (!v) return;
  if (v.status === "working" && !(await api.ask(`${v.name} is working. Close it?`, "Close"))) return;
  const next = neighborOf(v.id);
  await api.close(v.id).catch(report);
  terms.dispose(v.id);
  setTree(remove(tree, v.id), next);
}

/** A background job in a workspace of its own, attached in a new terminal. */
async function openJob(id: string) {
  try {
    const v = await api.openJob(id);
    jobs = jobs.filter((j) => j.id !== id);
    await show(v.id);
  } catch (e) {
    report(e);
  }
}

/** A copy of the session's conversation, running in a pane to its right. */
async function forkSession(id = owner(focusId)) {
  if (!id) return;
  try {
    const v = await api.fork(id);
    spaces = without(spaces, v.id, tree); // its sessions_changed may have given it a workspace already
    if (paneIds(tree).includes(id)) setTree(split(tree, id, v.id, "right"), v.id);
    else void show(v.id);
  } catch (e) {
    report(e);
  }
}

/** Files dropped on a terminal are pasted as their paths, as Terminal does; Claude attaches images it sees that way. */
async function dropFiles(id: string, files: File[], uriList: string) {
  try {
    const known = pathsFromUris(uriList); // the real paths, when the webview passes them on
    const paths = known.length
      ? known
      : await Promise.all(files.map(async (f) => api.saveDrop(safeName(f.name), new Uint8Array(await f.arrayBuffer()))));
    if (!paths.length) return;
    focusPane(id);
    terms.paste(id, paths.map(escapePath).join(" ") + " ");
  } catch (e) {
    report(e);
  }
}

// A file let go anywhere nothing takes it would make the webview open it in place of the app.
for (const type of ["dragover", "drop"]) {
  window.addEventListener(type, (e) => {
    if ((e as DragEvent).dataTransfer?.types.includes("Files")) e.preventDefault();
  });
}

/** Open the session's diff pane to its right, or close it if it's open. */
function toggleDiff(id = focusId) {
  if (!id) return;
  const d = diffId(owner(id));
  if (paneIds(tree).includes(d)) setTree(remove(tree, d), owner(id));
  else if (paneIds(tree).includes(owner(id))) setTree(split(tree, owner(id), d, "right"), owner(id));
}

/** A session dragged out of its workspace (a sidebar row or a pane's header) gets one of its own,
 *  just before or after `target`'s workspace, or last. */
function popOut(id: string, target: string | null, after: boolean) {
  const from = spaceOf(id);
  if (isDiff(id) || from < 0) return;
  let mine = spaces[from];
  if (paneIds(mine).filter((x) => !isDiff(x)).length > 1) {
    const rest = remove(remove(mine, id), diffId(id))!;
    if (mine === tree) setTree(rest, focusId === id ? neighborOf(id) : focusId);
    else spaces[from] = rest;
    mine = { pane: id };
    spaces.push(mine);
  }
  const to = target && target !== id ? spaceOf(target) : -1;
  const i = spaces.indexOf(mine);
  if (to >= 0) spaces = moveSpace(spaces, i, to, after);
  else spaces.push(...spaces.splice(i, 1)); // last
  drawSidebar();
  persist();
}

/** Two sessions of one workspace trade places, on screen and in the sidebar. */
function swapPanes(id: string, target: string) {
  const i = spaceOf(id);
  if (id === target || i < 0 || i !== spaceOf(target)) return;
  const t = swap(spaces[i], id, target)!;
  if (spaces[i] === tree) return setTree(t, focusId);
  spaces[i] = t;
  drawSidebar();
  persist();
}

/** ⌘⇧W: a diff pane closes; a session moves out into a workspace of its own, just below. */
function removePane() {
  const id = focusId;
  if (!id) return;
  if (isDiff(id)) return setTree(remove(tree, id), neighborOf(id));
  if (paneIds(tree).filter((x) => !isDiff(x)).length < 2) return; // already on its own
  setTree(remove(remove(tree, id), diffId(id)), neighborOf(id));
  spaces.splice(spaces.indexOf(tree!) + 1, 0, { pane: id });
  drawSidebar();
  persist();
}

function moveFocus(dir: Side) {
  if (!focusId) return;
  if (zoomed) {
    zoomed = null;
    draw(focusId);
  }
  const n = neighbor(panes.rects(), focusId, dir);
  if (n) focusPane(n);
}

/** Next session that needs you, else the next one you haven't looked at since it finished. */
function jump(order: string[]) {
  const start = focusId ? order.indexOf(owner(focusId)) : -1;
  const next = (want: (id: string) => boolean) => {
    for (let i = 1; i <= order.length; i++) {
      const id = order[(start + i + order.length) % order.length];
      if (want(id)) return id;
    }
  };
  const needs = new Set(views.filter((v) => v.status === "needs_you").map((v) => v.id));
  const id = next((id) => needs.has(id));
  const job = jobs.find((j) => j.status === "needs_you");
  if (id) void show(id);
  else if (job) void openJob(job.id); // waiting in the background: open it
  else {
    const u = next((id) => unread.has(id));
    if (u) void show(u);
  }
}

/** ⌘⇧↩: the focused pane fills the window, or the workspace comes back. */
function toggleZoom() {
  if (!focusId || paneIds(tree).length < 2) return;
  zoomed = zoomed ? null : focusId;
  draw(focusId);
}

/** ⌘F in the focused terminal (a diff pane has nothing to search). */
function findInPane() {
  if (focusId && !isDiff(focusId)) openFind(focusId, refocus);
}

/** ⌘⇧P: every command, then every session and background job. */
function openCommands() {
  if (overlayOpen()) return;
  const keyed = commands.filter((c) => c.label && c.chord.key !== "" && c.keys !== "⌘⇧P"); // not the listed-only ones
  const items: Item[] = [
    ...keyed.map((c) => ({ label: c.label, hint: c.keys, run: () => c.run(new KeyboardEvent("keydown")) })),
    ...views.map((v) => ({ label: `Go to ${v.name}`, hint: v.status === "needs_you" ? "needs you" : v.repo, run: () => void show(v.id) })),
    ...jobs.map((j) => ({ label: `Open job ${j.name}`, hint: j.status === "needs_you" ? "needs you" : "background job", run: () => void openJob(j.id) })),
  ];
  openPalette(items, refocus);
}

/** Edit in place: in the pane header when given, else in the sidebar row. */
function renameRow(id: string | null, inHeader?: HTMLElement) {
  const v = byId(id);
  const nameEl = inHeader ?? (id ? list.querySelector<HTMLElement>(`.row[data-id="${CSS.escape(id)}"] .name`) : null);
  if (v && nameEl) sidebar.startRename(nameEl, v, (rid, name) => void api.rename(rid, name).catch(report));
}

const overlayOpen = () => document.querySelector(".overlay") !== null;

/** Every app shortcut: run from here, and listed in Settings → Keyboard Shortcuts. */
const k = (key: string | string[], label: string, group: string, run: (e: KeyboardEvent) => void, more: Partial<Chord> = {}): Command => ({
  chord: { key, ...more },
  keys: "",
  label,
  group,
  run,
});
const ARROW_GLYPH: Record<string, string> = { ArrowLeft: "←", ArrowRight: "→", ArrowUp: "↑", ArrowDown: "↓" };
const commands: Command[] = [
  { ...k("n", "New terminal", "Sessions", () => void newSession()), keys: "⌘N" },
  { ...k("o", "Resume a past conversation from this folder", "Sessions", () => void resumePast()), keys: "⌘O" },
  { ...k("f", "Fork the conversation into a pane beside it", "Sessions", () => void forkSession(), { shift: true }), keys: "⌘⇧F" },
  { ...k("g", "What Claude changed (diff pane)", "Sessions", () => toggleDiff()), keys: "⌘G" },
  { ...k("j", "Next session or background job that needs you, then the next unread", "Sessions", () => jump(groups().flat())), keys: "⌘J" },
  { ...k("r", "Rename the session", "Sessions", () => renameRow(owner(focusId)), { shift: true }), keys: "⌘⇧R" },
  { ...k("w", "Close the pane (ends its session)", "Sessions", () => void closeFocused()), keys: "⌘W" },
  { ...k("f", "Find in the terminal", "In the terminal", () => findInPane()), keys: "⌘F" },
  { ...k("k", "Clear the terminal and its scrollback", "In the terminal", () => focusId && !isDiff(focusId) && terms.clear(focusId)), keys: "⌘K" },
  { ...k("d", "Split right with a new terminal", "Workspaces", () => void splitFocused("right")), keys: "⌘D" },
  { ...k("d", "Split down with a new terminal", "Workspaces", () => void splitFocused("bottom"), { shift: true }), keys: "⌘⇧D" },
  { ...k("w", "Move the pane into a workspace of its own", "Workspaces", () => removePane(), { shift: true }), keys: "⌘⇧W" },
  { ...k("Enter", "Zoom the pane to fill the window, or back", "Workspaces", () => toggleZoom(), { shift: true }), keys: "⌘⇧↩" },
  ...Object.entries(ARROWS).map(([key, side], i) => ({
    ...k(key, i ? "" : "Focus the pane that way", "Workspaces", () => moveFocus(side), { alt: true }),
    keys: i ? "" : `⌘⌥${Object.values(ARROW_GLYPH).join("")}`,
  })),
  {
    ...k("", "Go to workspace 1 to 9", "Workspaces", (e) => {
      const i = Number(e.code.slice(5)) - 1;
      if (spaces[i] && spaces[i] !== tree) showSpace(i, null);
    }, { code: /^Digit[1-9]$/ }),
    keys: "⌘1…9",
  },
  { ...k("b", "Collapse or expand the sidebar", "View", () => setRail(!document.body.classList.contains("rail"))), keys: "⌘B" },
  { ...k("t", "Choose a theme", "View", () => openThemes(), { shift: true }), keys: "⌘⇧T" },
  { ...k(["=", "+"], "Bigger text", "View", () => themes.zoom(1), { shift: null }), keys: "⌘=" },
  { ...k("-", "Smaller text", "View", () => themes.zoom(-1)), keys: "⌘-" },
  { ...k("0", "Text size from your config", "View", () => themes.zoom(0)), keys: "⌘0" },
  { ...k("p", "All commands, sessions and jobs", "App", () => openCommands(), { shift: true }), keys: "⌘⇧P" },
  { ...k(",", "Settings", "App", () => openSettings()), keys: "⌘," },
  { ...k("/", "Keyboard shortcuts", "App", () => openSettings("shortcuts")), keys: "⌘/" },
  // the menu handles ⌘Q; listed here only (an empty key never matches)
  { ...k("", "Quit (sessions keep running, unless Settings says otherwise)", "App", () => {}), keys: "⌘Q" },
];

window.addEventListener("keydown", (e) => {
  if (!e.metaKey || overlayOpen()) return;
  const c = commands.find((c) => matches(c.chord, e));
  if (!c) return;
  e.preventDefault();
  c.run(e);
});

const refocus = () => focusId && terms.focus(focusId);
const openThemes = () => void themes.openThemes(refocus);

function openSettings(section: Section = "general") {
  if (overlayOpen()) return;
  openPrefs(
    {
      settings,
      commands,
      rail: document.body.classList.contains("rail"),
      setRail,
      change: (s) => {
        settings = s;
        void api.setSettings(s).catch(report);
      },
      openThemes,
      closed: refocus,
    },
    section,
  );
}
void api.onMenu((item) => openSettings(item === "shortcuts" ? "shortcuts" : "general"));

api.onSessions((v) => {
  for (const x of v) track(byId(x.id), x);
  for (const id of unread) if (!v.some((x) => x.id === id)) unread.delete(id);
  views = v;
  const ids = views.map((x) => x.id);
  const placed = place(spaces, keep(ids), ids, parentOf);
  if (JSON.stringify(placed) === JSON.stringify(spaces)) {
    panes.updateHeaders(views, shell);
    return drawSidebar();
  }
  const i = spaces.indexOf(tree!);
  const before = paneIds(tree);
  spaces = placed;
  const here = spaces.findIndex((t) => paneIds(t).some((id) => before.includes(id)));
  if (here >= 0) {
    tree = spaces[here];
    draw(focusId);
  } else showSpace(Math.max(0, i - 1), null); // the workspace on screen went: show the one above, as setTree does
});
api.onSession((v) => {
  track(byId(v.id), v);
  views = views.map((x) => (x.id === v.id ? v : x));
  panes.updateHeaders(views, shell);
  drawSidebar();
});
api.onJobs((j) => {
  jobs = j;
  drawSidebar();
});
setInterval(drawSidebar, 10_000);
window.addEventListener("focus", () => markRead(focusId));
try {
  setRail(localStorage.getItem("roost.rail") === "1");
} catch {
  // no storage: start with the full sidebar
}
document.getElementById("new")!.onclick = () => void newSession();
initTips();
terms.setLinks((id) => byId(id)?.workdir ?? "", report);

await themes.init();
const boot = await api.boot();
views = boot.sessions;
recentRepos = boot.recentRepos;
claudeFound = boot.claudeFound;
shell = boot.shell;
settings = boot.settings ?? settings;
jobs = boot.jobs ?? [];
themes.setClaudeMatching(settings.claudeMatchesTheme);
// Roost before workspaces saved one layout; it becomes the first workspace.
const saved = boot.layout as { spaces?: Tree[] } | Tree;
const raw = saved && "spaces" in saved ? (saved.spaces ?? []) : [saved as Tree];
const live = views.filter((v) => v.status !== "exited").map((v) => v.id);
spaces = place(raw, keep(live), views.map((v) => v.id), parentOf);
showSpace(Math.max(0, boot.focus ? spaceOf(boot.focus) : 0), boot.focus);
