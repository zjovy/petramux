import { DRAG_TYPE } from "./panes";
import type { View } from "./api";

export function age(since: number): string {
  const s = Math.max(0, (Date.now() - since) / 1000);
  if (s < 60) return `${Math.floor(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

export interface Handlers {
  select(id: string): void;
  rename(id: string, name: string): void;
  /** Move the workspace holding `id` just before or after the one holding `target`. */
  reorder(id: string, target: string, after: boolean): void;
  /** A pane dragged here from its workspace: give it one of its own, by `target`'s (or last). */
  popOut(id: string, target: string | null, after: boolean): void;
}

/** A pane's header being dragged (not a sidebar row): the sidebar takes it as "on its own". */
const paneDrag = (e: DragEvent) => dragging === null && !!e.dataTransfer?.types.includes(DRAG_TYPE);

/** The workspace being dragged, if the drag started in the sidebar. */
let dragging: { id: string; ws: number } | null = null;
window.addEventListener("dragend", () => (dragging = null));
window.addEventListener("drop", () => (dragging = null));

/** One block per workspace, in order; the one on screen is lit, and several sessions share a card. */
export function render(
  el: HTMLElement,
  views: View[],
  groups: string[][],
  here: number,
  focusId: string | null,
  unread: Set<string>,
  on: Handlers,
) {
  el.replaceChildren();
  // a pane let go on the sidebar's empty space gets a workspace at the end
  el.ondragover = (e) => {
    if (paneDrag(e)) e.preventDefault();
  };
  el.ondrop = (e) => {
    if (!paneDrag(e)) return;
    e.preventDefault();
    on.popOut(e.dataTransfer!.getData(DRAG_TYPE), null, true);
  };
  const byId = new Map(views.map((v) => [v.id, v]));
  groups.forEach((ids, ws) => {
    const block = document.createElement("div");
    block.className = `ws${ws === here ? " here" : ""}${ids.length > 1 ? " multi" : ""}`;
    if (ids.length > 1) {
      const label = document.createElement("div");
      label.className = "wsh";
      label.textContent = `workspace · ${ids.length}`;
      block.append(label);
    }
    for (const id of ids) {
      const v = byId.get(id);
      if (v) block.append(row(v, ws, block, focusId, unread, on, v.parent != null && byId.has(v.parent)));
    }
    if (block.querySelector(".row")) el.append(block);
  });
}

function row(v: View, ws: number, block: HTMLElement, focusId: string | null, unread: Set<string>, on: Handlers, child: boolean) {
  const row = document.createElement("div");
  const isUnread = unread.has(v.id);
  row.className = `row s-${v.status}${v.id === focusId ? " on" : ""}${isUnread ? " unread" : ""}`;
  row.dataset.id = v.id;
  row.title = [v.name, v.note, v.repo].filter(Boolean).join("\n");
  const line = span("line", "");
  if (child) {
    row.style.paddingLeft = "22px";
    line.append(span("tree", "└"));
  }
  const name = span("name", v.name);
  const meta = span("meta", v.status === "needs_you" ? "" : age(v.since));
  if (v.status === "needs_you") meta.append(span("pill", "needs you"));
  line.append(span("dot", ""), name);
  if (isUnread && v.status !== "needs_you") line.append(span("ud", "")); // the pill already says so
  line.append(meta);
  row.append(line);
  if (v.note) {
    const note = span("note", v.note);
    note.style.paddingLeft = `${15 + (child ? 14 : 0)}px`;
    row.append(note);
  }
  row.onclick = () => on.select(v.id);
  row.ondblclick = () => {
    if (!document.body.classList.contains("rail")) startRename(name, v, on.rename);
  };
  row.draggable = true;
  row.addEventListener("dragstart", (e) => {
    e.dataTransfer?.setData(DRAG_TYPE, v.id);
    dragging = { id: v.id, ws };
  });
  const other = (e: DragEvent) => (dragging !== null && dragging.ws !== ws) || paneDrag(e);
  const clear = () => block.classList.remove("drop-before", "drop-after");
  row.addEventListener("dragover", (e) => {
    if (!other(e)) return;
    e.preventDefault();
    const r = block.getBoundingClientRect();
    const after = e.clientY > r.top + r.height / 2;
    block.classList.toggle("drop-before", !after);
    block.classList.toggle("drop-after", after);
  });
  row.addEventListener("dragleave", (e) => {
    if (!block.contains(e.relatedTarget as Node | null)) clear();
  });
  row.addEventListener("drop", (e) => {
    if (!other(e)) return;
    e.preventDefault();
    e.stopPropagation(); // the list's own drop is for empty space
    const after = block.classList.contains("drop-after");
    clear();
    const from = dragging;
    dragging = null; // this drop doesn't reach the window's reset
    if (from) on.reorder(from.id, v.id, after);
    else on.popOut(e.dataTransfer!.getData(DRAG_TYPE), v.id, after);
  });
  return row;
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
