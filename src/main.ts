import { api, type View } from "./api";
import { diffId, isDiff, keep, owner as ownerOf } from "./diff";
import { move, neighbor, panes as paneIds, prune, remove, replace, setRatio, split, type Side, type Tree } from "./layout";
import * as panes from "./panes";
import { openPicker } from "./picker";
import { openSheet } from "./sheet";
import * as sidebar from "./sidebar";
import * as terms from "./terminals";

const list = document.getElementById("list")!;
const host = document.getElementById("terms")!;
const empty = document.getElementById("empty")!;

let views: View[] = [];
let tree: Tree = null;
let focusId: string | null = null;
let recentRepos: string[] = [];
let claudeFound = true;
let shell = "zsh";
const unread = new Set<string>();

const byId = (id: string | null) => views.find((v) => v.id === id);
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
  saveTimer = window.setTimeout(() => void api.saveLayout(tree, focusId).catch(report), 300);
}

const hooks: panes.Hooks = {
  focus: (id) => focusPane(id),
  rename: (id, nameEl) => renameRow(id, nameEl),
  drop: (id, target, zone) => {
    setTree(move(tree, id, target, zone), id);
    if (!isDiff(id)) void api.select(id).catch(report);
  },
  diff: (id) => toggleDiff(id),
  ratio: (path, r) => {
    tree = setRatio(tree, path, r);
    persist();
  },
};

function drawSidebar() {
  if (list.querySelector(".rename")) return; // never wipe a rename in progress
  sidebar.render(list, views, owner(focusId), new Set(paneIds(tree)), unread, {
    select: (id) => void show(id),
    rename: (id, name) => void api.rename(id, name).catch(report),
  });
}

function drawEmpty() {
  empty.hidden = tree !== null;
  empty.textContent = claudeFound
    ? "Press ⌘N to start a session."
    : "Claude Code not found: terminals work, `claude` won't. Install it, or set ROOST_CLAUDE_BIN.";
}

/** Every layout change goes through here: render, redraw the sidebar, focus, save. */
function setTree(t: Tree, focus: string | null) {
  tree = t;
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

/** Focus the row's pane if it's on screen, else show it in the focused pane. */
async function show(id: string) {
  if (paneIds(tree).includes(id)) focusPane(id);
  else setTree(tree && focusId ? replace(tree, focusId, id) : { pane: id }, id);
  try {
    await api.select(id);
  } catch (e) {
    report(e);
  }
}

function defaultRepo(): string | undefined {
  return byId(owner(focusId))?.repo ?? recentRepos[0];
}

async function newSession() {
  const repo = defaultRepo() ?? (await api.pickFolder());
  if (!repo) return;
  openSheet({
    repo,
    recentRepos,
    start: async (prompt, repo, worktree) => {
      try {
        const v = await api.create(prompt, repo, worktree);
        recentRepos = [repo, ...recentRepos.filter((r) => r !== repo)].slice(0, 10);
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

function removePane() {
  if (!focusId) return;
  setTree(remove(tree, focusId), neighborOf(focusId));
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
  const order = sidebar.ordered(views).map((x) => x.view.id);
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
    const id = order[Number(e.code.slice(5)) - 1];
    if (id) void show(id);
  } else handled = false;
  if (handled) e.preventDefault();
});

api.onSessions((v) => {
  for (const x of v) track(byId(x.id), x);
  for (const id of unread) if (!v.some((x) => x.id === id)) unread.delete(id);
  views = v;
  const pruned = prune(tree, keep(views.map((x) => x.id)));
  if (JSON.stringify(pruned) !== JSON.stringify(tree)) setTree(pruned, focusId);
  else {
    panes.updateHeaders(views, shell);
    drawSidebar();
  }
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
setTree(prune(boot.layout, keep(views.filter((v) => v.status !== "exited").map((v) => v.id))), boot.focus);
for (const id of paneIds(tree)) if (!isDiff(id)) void api.select(id).catch(report);
