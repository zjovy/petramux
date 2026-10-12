import type { View } from "./api";
import { panes as paneIds, type Node, type Rect, type Tree, type Zone } from "./layout";
import * as terms from "./terminals";

export interface Hooks {
  focus(id: string): void;
  rename(id: string): void;
  drop(id: string, target: string, zone: Zone): void;
  ratio(path: ("a" | "b")[], ratio: number): void;
}

let host: HTMLElement | null = null;

const div = (cls: string) => {
  const d = document.createElement("div");
  d.className = cls;
  return d;
};

const running = (v: View) => !["shell", "asleep", "exited"].includes(v.status);

function header(el: HTMLElement, v: View | undefined, shell: string) {
  el.className = v ? `ph s-${v.status}` : "ph";
  const name = document.createElement("span");
  name.className = "name";
  name.textContent = v?.name ?? "";
  const what = document.createElement("span");
  what.className = "what";
  what.textContent = v ? (running(v) ? "claude" : shell) : "";
  el.replaceChildren(div("dot"), name, what);
}

/** Rebuilds the pane tree; terminals move into their pane bodies, the rest are parked. */
export function render(el: HTMLElement, tree: Tree, focus: string | null, views: View[], shell: string, hooks: Hooks) {
  host = el;
  const ids = paneIds(tree);
  for (const id of terms.ids()) if (!ids.includes(id)) terms.park(id);
  el.replaceChildren();
  if (!tree) return;
  const bodies = new Map<string, HTMLElement>();
  const build = (n: Node, path: ("a" | "b")[]): HTMLElement => {
    if ("pane" in n) {
      const pane = div(n.pane === focus ? "pane focus" : "pane");
      pane.dataset.id = n.pane;
      const head = div("ph");
      const body = div("pb");
      pane.append(head, body);
      header(head, views.find((v) => v.id === n.pane), shell);
      pane.addEventListener("mousedown", () => hooks.focus(n.pane), true);
      head.ondblclick = () => hooks.rename(n.pane);
      bodies.set(n.pane, body);
      return pane;
    }
    const box = div(`split split-${n.split}`);
    const a = build(n.a, [...path, "a"]);
    const b = build(n.b, [...path, "b"]);
    a.style.flex = `${n.ratio} 1 0`;
    b.style.flex = `${1 - n.ratio} 1 0`;
    const divider = div("divider");
    divider.dataset.path = path.join("");
    box.append(a, divider, b);
    return box;
  };
  el.append(build(tree, []));
  for (const [id, body] of bodies) terms.mount(id, body);
  requestAnimationFrame(() => ids.forEach(terms.refit));
}

const allPanes = () => [...(host?.querySelectorAll<HTMLElement>(".pane") ?? [])];

export function updateHeaders(views: View[], shell: string) {
  for (const pane of allPanes()) header(pane.querySelector(".ph")!, views.find((v) => v.id === pane.dataset.id), shell);
}

export function setFocus(id: string) {
  for (const pane of allPanes()) pane.classList.toggle("focus", pane.dataset.id === id);
}

export function rects(): Record<string, Rect> {
  const out: Record<string, Rect> = {};
  for (const pane of allPanes()) {
    const r = pane.getBoundingClientRect();
    out[pane.dataset.id!] = { left: r.left, top: r.top, width: r.width, height: r.height };
  }
  return out;
}
