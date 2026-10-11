import { api, type View } from "./api";
import * as sidebar from "./sidebar";
import * as terms from "./terminals";
import { openPicker } from "./picker";
import { openSheet } from "./sheet";

const list = document.getElementById("list")!;
const empty = document.getElementById("empty")!;

let views: View[] = [];
let selected: string | null = null;
let recentRepos: string[] = [];
let claudeFound = true;

function report(err: unknown) {
  console.error(err);
  const t = document.createElement("div");
  t.className = "toast";
  t.textContent = String(err);
  document.body.append(t);
  setTimeout(() => t.remove(), 4000);
}

function button(label: string, primary: boolean, onClick: () => void) {
  const b = document.createElement("button");
  b.className = primary ? "btn primary" : "btn";
  b.textContent = label;
  b.onclick = onClick;
  return b;
}

function drawSidebar() {
  sidebar.render(list, views, selected, {
    select: (id) => void select(id),
    rename: (id, name) => void api.rename(id, name).catch(report),
  });
}

function drawMain() {
  const v = views.find((x) => x.id === selected);
  empty.replaceChildren();
  empty.className = "";
  empty.hidden = false;
  if (!claudeFound) {
    terms.hideAll();
    empty.textContent = "Claude Code not found. Install it, or set ROOST_CLAUDE_BIN.";
  } else if (!v) {
    terms.hideAll();
    empty.textContent = views.length ? "Select a session." : "Press ⌘N to start a session.";
  } else if (v.resumeFailed) {
    empty.className = "card";
    const card = document.createElement("div");
    card.append(
      "Couldn't resume this session.",
      button("Start fresh here", true, () => void startFresh(v.id)),
      button("Remove", false, () => void closeSession(v.id)),
    );
    empty.append(card);
  } else {
    empty.hidden = true;
  }
}

function draw() {
  drawSidebar();
  drawMain();
}

async function select(id: string) {
  selected = id;
  terms.show(id);
  draw();
  try {
    await api.select(id);
  } catch (e) {
    report(e);
  }
}

async function closeSession(id: string) {
  const v = views.find((x) => x.id === id);
  if (!v) return;
  if (v.status === "working" && !(await api.ask(`${v.name} is working. Close it?`, "Close"))) return;
  const order = sidebar.ordered(views).map((x) => x.view.id);
  const next = order[order.indexOf(id) + 1] ?? order[order.indexOf(id) - 1] ?? null;
  await api.close(id).catch(report);
  terms.dispose(id);
  if (selected === id) {
    selected = null;
    if (next && next !== id) void select(next);
  }
}

async function startFresh(id: string) {
  try {
    const nv = await api.startFresh(id);
    terms.dispose(id);
    await select(nv.id);
  } catch (e) {
    report(e);
  }
}

function jumpNeedsYou(order: string[]) {
  const needs = new Set(views.filter((v) => v.status === "needs_you").map((v) => v.id));
  const start = selected ? order.indexOf(selected) : -1;
  for (let i = 1; i <= order.length; i++) {
    const id = order[(start + i + order.length) % order.length];
    if (needs.has(id)) return void select(id);
  }
}

function step(order: string[], delta: number) {
  if (!order.length) return;
  const i = selected ? order.indexOf(selected) : -1;
  void select(order[(i + delta + order.length) % order.length]);
}

function renameSelected() {
  const v = views.find((x) => x.id === selected);
  const nameEl = list.querySelector<HTMLElement>(".row.on .name");
  if (v && nameEl) sidebar.startRename(nameEl, v, (id, name) => void api.rename(id, name).catch(report));
}

function defaultRepo(): string | undefined {
  return views.find((x) => x.id === selected)?.repo ?? recentRepos[0];
}

async function newSession() {
  if (!claudeFound) return;
  const repo = defaultRepo() ?? (await api.pickFolder());
  if (!repo) return;
  openSheet({
    repo,
    recentRepos,
    start: async (prompt, repo, worktree) => {
      try {
        const v = await api.create(prompt, repo, worktree);
        recentRepos = [repo, ...recentRepos.filter((r) => r !== repo)].slice(0, 10);
        await select(v.id);
        return true;
      } catch (e) {
        report(e);
        return false;
      }
    },
    closed: () => terms.focusCurrent(),
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
        await select(v.id);
        return true;
      } catch (e) {
        report(e);
        return false;
      }
    },
    closed: () => terms.focusCurrent(),
  });
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
  else if (key === "w" && selected) void closeSession(selected);
  else if (key === "r" && e.shiftKey) renameSelected();
  else if (e.altKey && e.key === "ArrowUp") step(order, -1);
  else if (e.altKey && e.key === "ArrowDown") step(order, 1);
  else if (/^Digit[1-9]$/.test(e.code)) {
    const id = order[Number(e.code.slice(5)) - 1];
    if (id) void select(id);
  } else handled = false;
  if (handled) e.preventDefault();
});

api.onSessions((v) => {
  views = v;
  if (selected && !views.some((x) => x.id === selected)) selected = null;
  draw();
});
api.onSession((v) => {
  views = views.map((x) => (x.id === v.id ? v : x));
  draw();
});
setInterval(drawSidebar, 10_000);
document.getElementById("new")!.onclick = () => void newSession();

const boot = await api.boot();
views = boot.sessions;
recentRepos = boot.recentRepos;
claudeFound = boot.claudeFound;
draw();
