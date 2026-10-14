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

export interface Top {
  place: { dir: string; branch: string | null } | null;
  views: View[];
  jobs: Job[];
  needs(): void;
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
    const w = span("chip working", "");
    w.append(span("dot", ""), document.createTextNode(`${n.working} working`));
    status.append(w);
  }
}
