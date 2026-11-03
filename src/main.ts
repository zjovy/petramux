import { api, type Settings, type Status, type View } from "./api";
import { diffId, isDiff, keep, owner as ownerOf } from "./diff";
import { escapePath, pathsFromUris, safeName } from "./drop";
import { move, neighbor, panes as paneIds, remove, setRatio, split, swap, type Node, type Side, type Tree } from "./layout";
import * as panes from "./panes";
import { openPicker } from "./picker";
import * as sidebar from "./sidebar";
import { join, moveSpace, place, without } from "./spaces";
import * as themes from "./themes";
import { openPrefs, type Section } from "./prefs";
import { matches, type Chord, type Command } from "./shortcuts";
import * as terms from "./terminals";
import { openFind } from "./findbar";
import { openPalette, type Item } from "./palette";
import { initTips } from "./tip";
import * as topbar from "./topbar";

const list = document.getElementById("list")!;
const host = document.getElementById("terms")!;
const empty = document.getElementById("empty")!;

/** A session's own window (dragged out of the main one) shows just that session. */
const solo: string | null = (window as unknown as { __PETRAMUX_SESSION__?: string }).__PETRAMUX_SESSION__ ?? null;
if (solo) document.body.classList.add("solo");
/** The main window: sessions in windows of their own (not in its workspaces). */
const popped = new Set<string>();

let views: View[] = [];
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
  if (solo) return; // the main window's layout is the one kept
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => void api.saveLayout({ spaces, popped: [...popped] }, focusId).catch(report), 300);
}

/** Where the main window puts sessions: all but those in windows of their own. */
function placeAll(from: Tree[], ids: string[], live = ids): Node[] {
  const here = (list: string[]) => list.filter((id) => !popped.has(id));
  return place(from, keep(here(live)), here(ids), parentOf);
}

/** A session into a window of its own (where it was dropped, if it was): it leaves its workspace. */
async function popOutWindow(id: string, x?: number, y?: number) {
  if (solo || isDiff(id) || !byId(id)) return;
  if (!popped.has(id)) {
    const i = spaceOf(id);
    if (i >= 0) {
      const rest = remove(remove(spaces[i], id), diffId(id));
      if (spaces[i] === tree) setTree(rest, neighborOf(id));
      else if (rest) spaces[i] = rest;
      else spaces.splice(i, 1);
    }
    popped.add(id);
    drawSidebar();
    persist();
  }
  await api.openWindow(id, byId(id)?.name ?? "", x, y).catch(report);
}

/** A session in a window of its own, dragged back into the main window: it's the main window's
 *  again (in a workspace of its own, unless the drop puts it somewhere) and its window closes. */
function reclaim(id: string, ownSpace: boolean) {
  if (solo || !popped.delete(id)) return;
  if (ownSpace && byId(id) && spaceOf(id) < 0) spaces.push({ pane: id });
  void api.closeWindow(id).catch(report);
  drawSidebar();
  persist();
}

/** A pane's header or a sidebar row let go outside the window: that session gets a window. */
function dragEnd(id: string, e: DragEvent) {
  if (e.dataTransfer?.dropEffect !== "none" || (!e.screenX && !e.screenY)) return;
  const out =
    e.screenX < window.screenX ||
    e.screenY < window.screenY ||
    e.screenX > window.screenX + window.outerWidth ||
    e.screenY > window.screenY + window.outerHeight;
  if (out) void popOutWindow(id, e.screenX - 60, e.screenY - 20);
}

const hooks: panes.Hooks = {
  focus: (id) => focusPane(id),
  rename: (id, nameEl) => renameRow(id, nameEl),
  drop: (id, target, zone) => {
    reclaim(id, false); // dragged in from its own window: that window goes
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
  restart: (id) => void restartClaude(id),
  dragEnd,
  ratio: (path, r) => setTree(setRatio(tree, path, r), focusId),
};

/** Each workspace's sessions, in pane order (diff panes stand for nothing). */
const groups = () => spaces.map((t) => paneIds(t).filter((id) => !isDiff(id)));
const spaceOf = (id: string) => spaces.findIndex((t) => paneIds(t).includes(id));

/** The top bar: where the focused session works, and what needs you or is working. */
const whereEl = document.getElementById("where")!;
const statusEl = document.getElementById("status")!;
let shown: { id: string; at: number; value: { dir: string; branch: string | null } | null } = { id: "", at: 0, value: null };
function drawTop() {
  const id = owner(focusId);
  const show = () => topbar.render(whereEl, statusEl, { place: shown.id === id ? shown.value : null, views, needs: () => jump(groups().flat()), working: () => jump(groups().flat(), "working") });
  show();
  // the folder and branch: on a new focus, else at most every 5 s (a branch can change under Claude)
  if (!id || (shown.id === id && Date.now() - shown.at < 5000)) return;
  shown = { id, at: Date.now(), value: shown.id === id ? shown.value : null };
  void api.place(id).then(
    (value) => {
      if (shown.id === id) shown.value = value;
      show();
    },
    () => {},
  );
}

function drawSidebar() {
  drawTop();
  if (list.querySelector(".rename")) return; // never wipe a rename in progress
  sidebar.render(list, views, groups(), spaces.indexOf(tree!), owner(focusId), unread, [...popped].map(byId).filter((v): v is View => !!v), {
    focusWindow: (id) => void api.focusWindow(id),
    dragEnd,
    select: (id) => void show(id),
    rename: (id, name) => void api.rename(id, name).catch(report),
    popOut: (id, target, after) => popOut(id, target, after),
    swap: (id, target) => swapPanes(id, target),
    join: (id, target) => joinSpace(id, target),
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
  if (popped.has(id)) return void api.focusWindow(id); // in a window of its own
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

/** ⌥⌘N: a terminal under the focused session (its child in the sidebar), in the folder it works
 *  in, beside it. */
async function newChild() {
  const parent = owner(focusId);
  if (!parent || !focusId) return void newSession();
  try {
    const dir = await api.place(parent).then((p) => p.dir).catch(() => byId(parent)?.repo ?? "");
    const v = await api.create("", dir, false, parent);
    spaces = without(spaces, v.id, tree); // its sessions_changed may have given it a workspace already
    setTree(split(tree, focusId, v.id, "right"), v.id);
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
  // always asked: ⌘W sits next to ⌘Q and ⌘E, and closing ends what runs in the terminal
  const what = v.status === "working" ? `${v.name} is working. Close it?` : `Close ${v.name}?`;
  if (!(await api.ask(`${what}\n\nIts terminal ends. ⌘⇧T brings it back.`, "Close"))) return;
  const next = neighborOf(v.id);
  await api.close(v.id).catch(report);
  closedBeside.set(v.name, next);
  terms.dispose(v.id);
  setTree(remove(tree, v.id), next);
}

/** Where each closed session sat, by name: ⌘⇧T puts it back beside that pane, if it's still there. */
const closedBeside = new Map<string, string | null>();

/** ⌘⇧T: the last session closed comes back, as a browser brings back a closed tab. */
async function reopenClosed() {
  try {
    const v = await api.reopenClosed();
    const beside = closedBeside.get(v.name) ?? null;
    closedBeside.delete(v.name);
    spaces = without(spaces, v.id, tree); // its sessions_changed may have given it a workspace already
    const i = beside ? spaceOf(beside) : -1;
    if (i < 0) return void show(v.id);
    const t = split(spaces[i], beside!, v.id, "right")!;
    if (spaces[i] === tree) return setTree(t, v.id);
    spaces[i] = t;
    showSpace(i, v.id);
  } catch (e) {
    report(e);
  }
}

/** Claude stopped and started again on the same conversation, in its terminal. */
async function restartClaude(id = owner(focusId)) {
  const v = byId(id);
  if (!v) return;
  if (v.status === "working" && !(await api.ask(`${v.name} is working. Restart it? Its current turn stops.`, "Restart"))) return;
  await api.restartClaude(v.id).catch(report);
}

/** A copy of the session's conversation, running in a pane to its right. */
async function forkSession(id = owner(focusId)) {
  if (!id) return;
  try {
    const v = await api.fork(id);
    if (solo) return; // the copy goes to the main window's sidebar
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
  zoomed = null; // the diff shows beside its session
  if (paneIds(tree).includes(d)) setTree(remove(tree, d), owner(id));
  else if (paneIds(tree).includes(owner(id))) setTree(split(tree, owner(id), d, "right"), owner(id));
}

/** A session dragged out of its workspace (a sidebar row or a pane's header) gets one of its own,
 *  just before or after `target`'s workspace, or last. */
function popOut(id: string, target: string | null, after: boolean) {
  reclaim(id, true); // from its own window: back into a workspace, then placed as asked
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
  reclaim(id, true);
  const i = spaceOf(id);
  if (id === target || i < 0 || i !== spaceOf(target)) return;
  const t = swap(spaces[i], id, target)!;
  if (spaces[i] === tree) return setTree(t, focusId);
  spaces[i] = t;
  drawSidebar();
  persist();
}

/** A session dropped on another's row joins that workspace beside it, and that workspace shows. */
function joinSpace(id: string, target: string) {
  reclaim(id, true);
  const next = isDiff(id) ? spaces : join(spaces, id, target);
  if (next === spaces) return;
  spaces = next;
  showSpace(spaceOf(target), id);
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

/** Next session in that state; for "needs you", else the next
 *  one you haven't looked at since it finished. */
function jump(order: string[], status: Status = "needs_you") {
  const next = (want: (id: string) => boolean) => topbar.nextAfter(order, focusId ? owner(focusId) : null, want);
  const id = next((id) => byId(id)?.status === status);
  if (id) void show(id);
  else if (status === "needs_you") {
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

/** ⌘⇧P: every command, then every session. */
function openCommands() {
  if (overlayOpen()) return;
  // not the listed-only ones, nor the arrows or 1…9 (which way or which one is the key itself)
  const listedOnly = (c: Command) => c.chord.key === "" && !c.chord.code;
  const keyed = commands.filter(
    (c) => c.label && !listedOnly(c) && !(String(c.chord.key) in ARROWS) && !c.keys.includes("…") && c.keys !== "⌘⇧P",
  );
  const items: Item[] = [
    ...keyed.map((c) => ({ label: c.label, hint: c.keys, run: () => c.run(new KeyboardEvent("keydown")) })),
    ...views.map((v) => ({ label: `Go to ${v.name}`, hint: v.status === "needs_you" ? "needs you" : v.repo, run: () => void show(v.id) })),
    { label: "Restart Claude here", hint: "same conversation; picks up a Claude update", run: () => void restartClaude() },
    { label: "Restart petramux", hint: "sessions keep running; installs a newer build", run: () => void restart() },
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
  { ...k("j", "Next session that needs you, then the next unread", "Sessions", () => jump(groups().flat())), keys: "⌘J" },
  { ...k("r", "Rename the session", "Sessions", () => renameRow(owner(focusId)), { shift: true }), keys: "⌘⇧R" },
  { ...k("w", "Close the pane (ends its session)", "Sessions", () => void closeFocused()), keys: "⌘W" },
  { ...k("t", "Bring back the last closed session", "Sessions", () => void reopenClosed(), { shift: true }), keys: "⌘⇧T" },
  { ...k("f", "Find in the terminal", "In the terminal", () => findInPane()), keys: "⌘F" },
  { ...k("k", "Clear the terminal and its scrollback", "In the terminal", () => focusId && !isDiff(focusId) && terms.clear(focusId)), keys: "⌘K" },
  // by the key's place: ⌥N types a dead key (˜), so its letter never comes through
  { ...k("", "New terminal under this session (its child)", "Sessions", () => void newChild(), { alt: true, code: /^KeyN$/ }), keys: "⌥⌘N" },
  { ...k("d", "Split right with a new terminal", "Workspaces", () => void splitFocused("right")), keys: "⌘D" },
  { ...k("d", "Split down with a new terminal", "Workspaces", () => void splitFocused("bottom"), { shift: true }), keys: "⌘⇧D" },
  { ...k("w", "Move the pane into a workspace of its own", "Workspaces", () => removePane(), { shift: true }), keys: "⌘⇧W" },
  { ...k("o", "Open the pane in a window of its own (or drag it out)", "Workspaces", () => void popOutWindow(owner(focusId)), { shift: true }), keys: "⌘⇧O" },
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
  // by the key's place: ⌥T types a character (†), so its letter never comes through
  { ...k("", "Choose a theme", "View", () => openThemes(), { alt: true, code: /^KeyT$/ }), keys: "⌥⌘T" },
  { ...k(["=", "+"], "Bigger text", "View", () => themes.zoom(1), { shift: null }), keys: "⌘=" },
  { ...k("-", "Smaller text", "View", () => themes.zoom(-1)), keys: "⌘-" },
  { ...k("0", "Text size from your config", "View", () => themes.zoom(0)), keys: "⌘0" },
  { ...k("p", "All commands and sessions", "App", () => openCommands(), { shift: true }), keys: "⌘⇧P" },
  { ...k(",", "Settings", "App", () => openSettings()), keys: "⌘," },
  { ...k("/", "Keyboard shortcuts", "App", () => openSettings("shortcuts")), keys: "⌘/" },
  // the menu handles ⌘Q; listed here only (an empty key never matches)
  { ...k("", "Quit (sessions keep running, unless Settings says otherwise)", "App", () => {}), keys: "⌘Q" },
];

/** In a session's own window: what applies to that one pane (⌘W closes the window). */
const SOLO = new Set(["⌘F", "⌘K", "⌘G", "⌘=", "⌘-", "⌘0", "⌥⌘T", "⌘,", "⌘/", "⌘⇧↩"]);
window.addEventListener("keydown", (e) => {
  if (!e.metaKey || overlayOpen()) return;
  if (solo && e.key.toLowerCase() === "w" && !e.shiftKey && !e.altKey) {
    e.preventDefault();
    return void api.closeWindow(solo); // back to the main window, still running
  }
  const c = commands.find((c) => matches(c.chord, e) && (!solo || SOLO.has(c.keys)));
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
  if (solo) {
    if (!ids.includes(solo)) void api.closeWindow(solo); // its session ended
    panes.updateHeaders(views, shell);
    return drawSidebar();
  }
  for (const id of popped) if (!ids.includes(id)) popped.delete(id);
  const placed = placeAll(spaces, ids);
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
setInterval(drawSidebar, 10_000);
window.addEventListener("focus", () => markRead(focusId));
try {
  setRail(localStorage.getItem("roost.rail") === "1");
} catch {
  // no storage: start with the full sidebar
}
document.getElementById("new")!.onclick = () => void newSession();

/** A newer build is ready: the button to restart into it shows. */
const updateButton = document.getElementById("update")!;
async function checkUpdate() {
  updateButton.hidden = !(await api.updateReady().catch(() => false));
}
async function restart() {
  try {
    await api.restart(); // the app quits; its sessions keep running and it opens again
  } catch (e) {
    report(e);
  }
}
updateButton.onclick = () => void restart();
document.getElementById("search")!.onclick = () => openCommands();
document.getElementById("gear")!.onclick = () => openSettings();
void checkUpdate();
setInterval(() => void checkUpdate(), 30_000);
window.addEventListener("focus", () => void checkUpdate());
initTips();
terms.setLinks((id) => byId(id)?.workdir ?? "", report);

await themes.init();
const boot = await api.boot();
views = boot.sessions;
recentRepos = boot.recentRepos;
claudeFound = boot.claudeFound;
shell = boot.shell;
settings = boot.settings ?? settings;
terms.setRestored(boot.restored ?? []);
themes.setClaudeMatching(settings.claudeMatchesTheme);
if (solo) {
  // a session's own window: that session, alone; its screen is drawn afresh once replayed
  terms.setRestored([solo]);
  spaces = [{ pane: solo }];
  showSpace(0, solo);
} else {
  // Roost before workspaces saved one layout; it becomes the first workspace.
  const saved = boot.layout as { spaces?: Tree[]; popped?: string[] } | Tree;
  const raw = saved && "spaces" in saved ? (saved.spaces ?? []) : [saved as Tree];
  const kept = saved && "spaces" in saved ? (saved.popped ?? []) : [];
  for (const id of kept) if (byId(id)) popped.add(id);
  const live = views.filter((v) => v.status !== "exited").map((v) => v.id);
  spaces = placeAll(raw, views.map((v) => v.id), live);
  showSpace(Math.max(0, boot.focus ? spaceOf(boot.focus) : 0), boot.focus);
  for (const id of popped) void api.openWindow(id, byId(id)?.name ?? "").catch(report); // back where they were
  api.onWindowClosed((id) => {
    // its own window closed: the session (still running) comes back, in a workspace of its own
    if (!popped.delete(id)) return;
    if (byId(id) && spaceOf(id) < 0) spaces.push({ pane: id }); // never a second copy
    drawSidebar();
    persist();
  });
}
