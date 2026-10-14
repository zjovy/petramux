import type { View } from "./api";
import { panes as paneIds, zoneAt, type Node, type Rect, type Tree, type Zone } from "./layout";
import { isDiff, owner } from "./diff";
import * as diffpane from "./diffpane";
import * as terms from "./terminals";

export interface Hooks {
  focus(id: string): void;
  rename(id: string, nameEl?: HTMLElement): void;
  drop(id: string, target: string, zone: Zone): void;
  /** Files dropped from Finder onto a terminal. */
  files(id: string, files: File[], uriList: string): void;
  ratio(path: ("a" | "b")[], ratio: number): void;
  diff(id: string): void;
  /** Copy the session's conversation into a pane beside it. */
  fork(id: string): void;
  /** Claude again on the same conversation (a newer version, if one was installed). */
  restart(id: string): void;
  /** A header drag ended (let go outside the window: the session gets a window of its own). */
  dragEnd(id: string, e: DragEvent): void;
}

let host: HTMLElement | null = null;
let current: Hooks | null = null;

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
  if (!v) return;
  const button = (label: string, title: string, act: (id: string) => void, cls = "") => {
    const b = document.createElement("button");
    b.className = `hbtn ${cls}`;
    b.textContent = label;
    b.title = title;
    b.onmousedown = (e) => e.stopPropagation(); // a click, not the start of a header drag
    b.onclick = () => act(v.id);
    return b;
  };
  if (running(v)) {
    const restart = (id: string) => current?.restart(id);
    if (v.outdated) el.append(button("update", "A newer Claude is installed: restart this one on the same conversation", restart, "update"));
    else el.append(button("restart", "Restart Claude on the same conversation", restart));
    el.append(button("fork", "Copy this conversation into a pane beside it (⌘⇧F)", (id) => current?.fork(id)));
  }
  el.append(button("diff", "What changed (⌘G)", (id) => current?.diff(id)));
}

export const DRAG_TYPE = "text/roost-id";

const ZONE_BOX: Record<Zone, string> = {
  left: "left:0;top:22px;bottom:0;width:50%",
  right: "right:0;top:22px;bottom:0;width:50%",
  top: "left:0;right:0;top:22px;height:calc(50% - 11px)",
  bottom: "left:0;right:0;bottom:0;height:calc(50% - 11px)",
  center: "left:0;right:0;top:22px;bottom:0",
};

function dropTarget(pane: HTMLElement, id: string, hooks: Hooks) {
  let overlay: HTMLElement | null = null;
  const clear = () => {
    overlay?.remove();
    overlay = null;
  };
  const files = (e: DragEvent) => !isDiff(id) && !!e.dataTransfer?.types.includes("Files");
  pane.addEventListener("dragover", (e) => {
    if (files(e)) {
      e.preventDefault(); // files land in this terminal as pasted paths
      overlay ??= pane.appendChild(div("drop"));
      overlay.setAttribute("style", ZONE_BOX.center);
      overlay.dataset.zone = "";
      return;
    }
    if (!e.dataTransfer?.types.includes(DRAG_TYPE)) return;
    e.preventDefault();
    const r = pane.getBoundingClientRect();
    const zone = zoneAt({ left: r.left, top: r.top, width: r.width, height: r.height }, e.clientX, e.clientY);
    overlay ??= pane.appendChild(div("drop"));
    overlay.setAttribute("style", ZONE_BOX[zone]);
    overlay.dataset.zone = zone;
  });
  pane.addEventListener("dragleave", (e) => {
    if (!pane.contains(e.relatedTarget as HTMLElement | null)) clear();
  });
  pane.addEventListener("drop", (e) => {
    e.preventDefault();
    if (files(e)) {
      clear();
      // read now: a drop's data is gone once the event is over
      return hooks.files(id, [...e.dataTransfer!.files], e.dataTransfer!.getData("text/uri-list"));
    }
    const dragged = e.dataTransfer?.getData(DRAG_TYPE);
    const zone = overlay?.dataset.zone as Zone | undefined;
    clear();
    if (dragged && zone) hooks.drop(dragged, id, zone);
  });
}

function resizable(box: HTMLElement, divider: HTMLElement, dir: "row" | "col", path: ("a" | "b")[], hooks: Hooks) {
  const sides = () => [divider.previousElementSibling as HTMLElement, divider.nextElementSibling as HTMLElement];
  const apply = (ratio: number) => {
    const [a, b] = sides();
    a.style.flex = `${ratio} 1 0`;
    b.style.flex = `${1 - ratio} 1 0`;
  };
  divider.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const r = box.getBoundingClientRect();
    let ratio = 0.5;
    const onMove = (ev: MouseEvent) => {
      const raw = dir === "row" ? (ev.clientX - r.left) / r.width : (ev.clientY - r.top) / r.height;
      ratio = Math.min(0.9, Math.max(0.1, raw));
      apply(ratio);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      hooks.ratio(path, ratio); // terminals refit through their ResizeObservers
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });
  divider.addEventListener("dblclick", () => {
    apply(0.5);
    hooks.ratio(path, 0.5);
  });
}

/** Rebuilds the pane tree; terminals move into their pane bodies, the rest are parked. */
export function render(el: HTMLElement, tree: Tree, focus: string | null, views: View[], shell: string, hooks: Hooks) {
  host = el;
  current = hooks;
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
      if (isDiff(n.pane)) {
        diffpane.forget(n.pane);
        diffpane.update(pane, n.pane, views.find((v) => v.id === owner(n.pane)));
      } else {
        header(head, views.find((v) => v.id === n.pane), shell);
        bodies.set(n.pane, body);
      }
      pane.addEventListener("mousedown", () => hooks.focus(n.pane), true);
      head.ondblclick = () => {
        if (!isDiff(n.pane)) hooks.rename(n.pane, head.querySelector<HTMLElement>(".name") ?? undefined);
      };
      head.draggable = true;
      head.addEventListener("dragstart", (e) => {
        e.dataTransfer?.setData(DRAG_TYPE, n.pane);
        if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
      });
      head.addEventListener("dragend", (e) => hooks.dragEnd(n.pane, e));
      dropTarget(pane, n.pane, hooks);
      return pane;
    }
    const box = div(`split split-${n.split}`);
    const a = build(n.a, [...path, "a"]);
    const b = build(n.b, [...path, "b"]);
    a.style.flex = `${n.ratio} 1 0`;
    b.style.flex = `${1 - n.ratio} 1 0`;
    const divider = div("divider");
    divider.dataset.path = path.join("");
    resizable(box, divider, n.split, path, hooks);
    box.append(a, divider, b);
    return box;
  };
  el.append(build(tree, []));
  for (const [id, body] of bodies) terms.mount(id, body);
  requestAnimationFrame(() => ids.forEach(terms.refit));
}

const allPanes = () => [...(host?.querySelectorAll<HTMLElement>(".pane") ?? [])];

export function updateHeaders(views: View[], shell: string) {
  for (const pane of allPanes()) {
    const head = pane.querySelector<HTMLElement>(".ph")!;
    if (head.querySelector(".rename")) continue; // never wipe a rename in progress
    const id = pane.dataset.id!;
    if (isDiff(id)) diffpane.update(pane, id, views.find((v) => v.id === owner(id)));
    else header(head, views.find((v) => v.id === id), shell);
  }
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
