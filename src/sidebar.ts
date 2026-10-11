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

export function render(el: HTMLElement, views: View[], selected: string | null, on: Handlers) {
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
    row.className = `row s-${v.status}${v.id === selected ? " on" : ""}`;
    row.dataset.id = v.id;
    row.style.paddingLeft = `${8 + depth * 14}px`;
    if (depth) row.append(span("tree", "└"));
    const name = span("name", v.name);
    const meta = span("meta", v.status === "needs_you" ? "" : age(v.since));
    if (v.status === "needs_you") meta.append(span("pill", "needs you"));
    row.append(span("dot", ""), name, meta);
    row.onclick = () => on.select(v.id);
    row.ondblclick = () => startRename(name, v, on.rename);
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
