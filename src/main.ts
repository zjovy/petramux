import { api, type View } from "./api";
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

const byId = (id: string | null) => views.find((v) => v.id === id);
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
  rename: (id) => renameRow(id),
  drop: (id, target, zone) => {
    setTree(move(tree, id, target, zone), id);
    void api.select(id).catch(report);
  },
  ratio: (path, r) => {
    tree = setRatio(tree, path, r);
    persist();
  },
};

function drawSidebar() {
  if (list.querySelector(".rename")) return; // never wipe a rename in progress
  sidebar.render(list, views, focusId, new Set(paneIds(tree)), {
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
}

function focusPane(id: string) {
  if (focusId === id) return;
  focusId = id;
  panes.setFocus(id);
  terms.focus(id);
  drawSidebar();
  persist();
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
  return byId(focusId)?.repo ?? recentRepos[0];
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
  const src = byId(focusId);
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
  const v = byId(focusId);
  if (!v) return;
  if (v.status === "working" && !(await api.ask(`${v.name} is working. Close it?`, "Close"))) return;
  const next = neighborOf(v.id);
  await api.close(v.id).catch(report);
  terms.dispose(v.id);
  setTree(remove(tree, v.id), next);
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

function jumpNeedsYou(order: string[]) {
  const needs = new Set(views.filter((v) => v.status === "needs_you").map((v) => v.id));
  const start = focusId ? order.indexOf(focusId) : -1;
  for (let i = 1; i <= order.length; i++) {
    const id = order[(start + i + order.length) % order.length];
    if (needs.has(id)) return void show(id);
  }
}

function renameRow(id: string | null) {
  const v = byId(id);
  const nameEl = id ? list.querySelector<HTMLElement>(`.row[data-id="${CSS.escape(id)}"] .name`) : null;
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
  else if (key === "j") jumpNeedsYou(order);
  else if (key === "d") void splitFocused(e.shiftKey ? "bottom" : "right");
  else if (key === "w" && e.shiftKey) removePane();
  else if (key === "w") void closeFocused();
  else if (key === "r" && e.shiftKey) renameRow(focusId);
  else if (e.altKey && e.key in ARROWS) moveFocus(ARROWS[e.key]);
  else if (/^Digit[1-9]$/.test(e.code)) {
    const id = order[Number(e.code.slice(5)) - 1];
    if (id) void show(id);
  } else handled = false;
  if (handled) e.preventDefault();
});

api.onSessions((v) => {
  views = v;
  const pruned = prune(tree, new Set(views.map((x) => x.id)));
  if (JSON.stringify(pruned) !== JSON.stringify(tree)) setTree(pruned, focusId);
  else {
    panes.updateHeaders(views, shell);
    drawSidebar();
  }
});
api.onSession((v) => {
  views = views.map((x) => (x.id === v.id ? v : x));
  panes.updateHeaders(views, shell);
  drawSidebar();
});
setInterval(drawSidebar, 10_000);
document.getElementById("new")!.onclick = () => void newSession();

const boot = await api.boot();
views = boot.sessions;
recentRepos = boot.recentRepos;
claudeFound = boot.claudeFound;
shell = boot.shell;
setTree(prune(boot.layout, new Set(views.filter((v) => v.status !== "exited").map((v) => v.id))), boot.focus);
for (const id of paneIds(tree)) void api.select(id).catch(report);
