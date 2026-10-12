import { DRAG_TYPE } from "./panes";
import type { View } from "./api";

export function ordered(views: View[]): { view: View; depth: number }[] {
  const ids = new Set(views.map((v) => v.id));
  const parentOf = (v: View) => (v.parent && ids.has(v.parent) ? v.parent : null);
  const kids = (id: string | null) =>
    views.filter((v) => parentOf(v) === id).sort((a, b) => a.createdAt - b.createdAt);
  const groups = new Map<string, View[]>();
  for (const root of kids(null)) {
    const g = groups.get(root.repo) ?? [];
    g.push(root);
    groups.set(root.repo, g);
  }
  const out: { view: View; depth: number }[] = [];
  const walk = (v: View, depth: number) => {
    out.push({ view: v, depth });
    for (const k of kids(v.id)) walk(k, depth + 1);
  };
  for (const roots of groups.values()) for (const r of roots) walk(r, 0);
  return out;
}

export function age(since: number): string {
  const s = Math.max(0, (Date.now() - since) / 1000);
  if (s < 60) return `${Math.floor(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

const basename = (p: string) => p.split("/").filter(Boolean).pop() ?? p;

export interface Handlers {
  select(id: string): void;
  rename(id: string, name: string): void;
}

export function render(
  el: HTMLElement,
  views: View[],
  focusId: string | null,
  visible: Set<string>,
  unread: Set<string>,
  on: Handlers,
) {
  el.replaceChildren();
  let lastRepo: string | null = null;
  for (const { view: v, depth } of ordered(views)) {
    if (depth === 0 && v.repo !== lastRepo) {
      lastRepo = v.repo;
      const g = document.createElement("div");
      g.className = "grp";
      g.textContent = basename(v.repo);
      g.title = v.repo;
      el.append(g);
    }
    const row = document.createElement("div");
    const isUnread = unread.has(v.id);
    row.className = `row s-${v.status}${v.id === focusId ? " on" : ""}${visible.has(v.id) ? "" : " off"}${isUnread ? " unread" : ""}`;
    row.dataset.id = v.id;
    row.title = v.note ? `${v.name}\n${v.note}` : v.name;
    row.style.paddingLeft = `${8 + depth * 14}px`;
    const line = span("line", "");
    if (depth) line.append(span("tree", "└"));
    const name = span("name", v.name);
    const meta = span("meta", v.status === "needs_you" ? "" : age(v.since));
    if (v.status === "needs_you") meta.append(span("pill", "needs you"));
    line.append(span("dot", ""), name);
    if (isUnread && v.status !== "needs_you") line.append(span("ud", "")); // the pill already says so
    line.append(meta);
    row.append(line);
    if (v.note) {
      const note = span("note", v.note);
      note.style.paddingLeft = `${15 + (depth ? 14 : 0)}px`;
      row.append(note);
    }
    row.onclick = () => on.select(v.id);
    row.ondblclick = () => {
      if (!document.body.classList.contains("rail")) startRename(name, v, on.rename);
    };
    row.draggable = true;
    row.addEventListener("dragstart", (e) => e.dataTransfer?.setData(DRAG_TYPE, v.id));
    el.append(row);
  }
}

function span(cls: string, text: string) {
  const s = document.createElement("span");
  s.className = cls;
  s.textContent = text;
  return s;
}

export function startRename(nameEl: HTMLElement, v: View, done: (id: string, name: string) => void) {
  const input = document.createElement("input");
  input.className = "rename";
  input.maxLength = 60;
  input.value = v.name;
  nameEl.replaceWith(input);
  input.focus();
  input.select();
  let finished = false;
  const finish = (save: boolean) => {
    if (finished) return;
    finished = true;
    const value = input.value.trim();
    input.replaceWith(nameEl);
    if (save && value && value !== v.name) done(v.id, value);
  };
  input.onkeydown = (e) => {
    e.stopPropagation();
    if (e.key === "Enter") finish(true);
    if (e.key === "Escape") finish(false);
  };
  input.onblur = () => finish(true);
}
