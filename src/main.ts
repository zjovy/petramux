import { api, type View } from "./api";
import { diffId, isDiff, keep, owner as ownerOf } from "./diff";
import { move, neighbor, panes as paneIds, remove, setRatio, split, type Node, type Side, type Tree } from "./layout";
import * as panes from "./panes";
import { openPicker } from "./picker";
import * as sidebar from "./sidebar";
import { moveSpace, place, without } from "./spaces";
import * as terms from "./terminals";

const list = document.getElementById("list")!;
const host = document.getElementById("terms")!;
const empty = document.getElementById("empty")!;

let views: View[] = [];
let spaces: Node[] = []; // workspaces, in sidebar order
let tree: Tree = null; // the workspace on screen: always one of `spaces`, or null when there are none
let focusId: string | null = null;
let recentRepos: string[] = [];
let claudeFound = true;
let shell = "zsh";
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
    setTree(move(tree, id, target, zone), id);
    if (!isDiff(id)) void api.select(id).catch(report);
  },
  diff: (id) => toggleDiff(id),
  ratio: (path, r) => setTree(setRatio(tree, path, r), focusId),
};

/** Each workspace's sessions, in pane order (diff panes stand for nothing). */
const groups = () => spaces.map((t) => paneIds(t).filter((id) => !isDiff(id)));
const spaceOf = (id: string) => spaces.findIndex((t) => paneIds(t).includes(id));

function drawSidebar() {
  if (list.querySelector(".rename")) return; // never wipe a rename in progress
  sidebar.render(list, views, groups(), spaces.indexOf(tree!), owner(focusId), unread, {
    select: (id) => void show(id),
    rename: (id, name) => void api.rename(id, name).catch(report),
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
  panes.render(host, tree, focusId, views, shell, hooks);
  drawEmpty();
  drawSidebar();
  if (focusId) terms.focus(focusId);
  persist();
  markRead(focusId);
}

function focusPane(id: string) {
  if (focusId === id) return;
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
    const v = await api.create("", defaultRepo() ?? "", false);
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

/** Open the session's diff pane to its right, or close it if it's open. */
function toggleDiff(id = focusId) {
  if (!id) return;
  const d = diffId(owner(id));
  if (paneIds(tree).includes(d)) setTree(remove(tree, d), owner(id));
  else if (paneIds(tree).includes(owner(id))) setTree(split(tree, owner(id), d, "right"), owner(id));
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
  const id = next((id) => needs.has(id)) ?? next((id) => unread.has(id));
  if (id) void show(id);
}

/** Edit in place: in the pane header when given, else in the sidebar row. */
function renameRow(id: string | null, inHeader?: HTMLElement) {
  const v = byId(id);
  const nameEl = inHeader ?? (id ? list.querySelector<HTMLElement>(`.row[data-id="${CSS.escape(id)}"] .name`) : null);
  if (v && nameEl) sidebar.startRename(nameEl, v, (rid, name) => void api.rename(rid, name).catch(report));
}

const overlayOpen = () => document.querySelector(".overlay") !== null;

window.addEventListener("keydown", (e) => {
  if (!e.metaKey || overlayOpen()) return;
  const order = groups().flat();
  const key = e.key.toLowerCase();
  let handled = true;
  if (key === "n" && !e.shiftKey) void newSession();
  else if (key === "o") void resumePast();
  else if (key === "j") jump(order);
  else if (key === "b") setRail(!document.body.classList.contains("rail"));
  else if (key === "d") void splitFocused(e.shiftKey ? "bottom" : "right");
  else if (key === "w" && e.shiftKey) removePane();
  else if (key === "w") void closeFocused();
  else if (key === "r" && e.shiftKey) renameRow(owner(focusId));
  else if (key === "g") toggleDiff();
  else if (e.altKey && e.key in ARROWS) moveFocus(ARROWS[e.key]);
  else if (/^Digit[1-9]$/.test(e.code)) {
    const i = Number(e.code.slice(5)) - 1;
    if (spaces[i] && spaces[i] !== tree) showSpace(i, null);
  } else handled = false;
  if (handled) e.preventDefault();
});

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
setInterval(drawSidebar, 10_000);
window.addEventListener("focus", () => markRead(focusId));
try {
  setRail(localStorage.getItem("roost.rail") === "1");
} catch {
  // no storage: start with the full sidebar
}
document.getElementById("new")!.onclick = () => void newSession();

const boot = await api.boot();
views = boot.sessions;
recentRepos = boot.recentRepos;
claudeFound = boot.claudeFound;
shell = boot.shell;
// Roost before workspaces saved one layout; it becomes the first workspace.
const saved = boot.layout as { spaces?: Tree[] } | Tree;
const raw = saved && "spaces" in saved ? (saved.spaces ?? []) : [saved as Tree];
const live = views.filter((v) => v.status !== "exited").map((v) => v.id);
spaces = place(raw, keep(live), views.map((v) => v.id), parentOf);
showSpace(Math.max(0, boot.focus ? spaceOf(boot.focus) : 0), boot.focus);
