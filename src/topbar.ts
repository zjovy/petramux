import type { Job, Status, View } from "./api";

/** A folder as the top bar names it: its own name, or for a Claude worktree, its repo and itself. */
export function placeLabel(dir: string): string {
  const wt = dir.match(/^(.*)\/\.claude\/worktrees\/([^/]+)(?:\/(.+))?$/);
  if (wt) return [wt[1].split("/").pop(), wt[2], wt[3]].filter(Boolean).join(" › ");
  return dir.split("/").filter(Boolean).pop() ?? dir;
}

/** How many sessions and background jobs need you, and how many are working. */
export function counts(views: { status: Status }[], jobs: { status: Status }[]): { needs: number; working: number } {
  const all = [...views, ...jobs];
  return { needs: all.filter((x) => x.status === "needs_you").length, working: all.filter((x) => x.status === "working").length };
}

/** The first session after `from` in `order` (round the list) that `want` picks. */
export function nextAfter(order: string[], from: string | null, want: (id: string) => boolean): string | undefined {
  const start = from === null ? -1 : order.indexOf(from);
  for (let i = 1; i <= order.length; i++) {
    const id = order[(start + i + order.length) % order.length];
    if (want(id)) return id;
  }
}

export interface Top {
  place: { dir: string; branch: string | null } | null;
  views: View[];
  jobs: Job[];
  needs(): void;
  working(): void;
}

const span = (cls: string, text: string) => {
  const s = document.createElement("span");
  s.className = cls;
  s.textContent = text;
  return s;
};

/** Where the focused session works (left) and what needs you or is working (right). */
export function render(where: HTMLElement, status: HTMLElement, t: Top) {
  where.replaceChildren();
  where.title = t.place?.dir ?? "";
  if (t.place) {
    where.append(span("folder", placeLabel(t.place.dir)));
    if (t.place.branch) where.append(span("branch", `⎇ ${t.place.branch}`));
  }
  const n = counts(t.views, t.jobs);
  status.replaceChildren();
  if (n.needs) {
    const b = document.createElement("button");
    b.className = "chip needs";
    b.title = "Go to the next one (⌘J)";
    b.append(span("dot", ""), document.createTextNode(`${n.needs} need${n.needs === 1 ? "s" : ""} you`));
    b.onclick = t.needs;
    status.append(b);
  }
  if (n.working) {
    const w = document.createElement("button");
    w.className = "chip working";
    w.title = "Go to the next one";
    w.append(span("dot", ""), document.createTextNode(`${n.working} working`));
    w.onclick = t.working;
    status.append(w);
  }
}
