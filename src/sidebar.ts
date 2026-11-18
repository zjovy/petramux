import { DRAG_TYPE } from "./panes";
import type { View } from "./api";
import { nested } from "./spaces";

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
  /** One session out of its workspace into one of its own, by `target`'s (or last). */
  popOut(id: string, target: string | null, after: boolean): void;
  /** Two sessions of one workspace trade places. */
  swap(id: string, target: string): void;
  /** A session dropped on the middle of another workspace's row joins that workspace, beside it. */
  join(id: string, target: string): void;
  /** A session in a window of its own: bring that window to the front. */
  focusWindow(id: string): void;
  /** A row drag ended (let go outside the window: the session gets a window of its own). */
  dragEnd(id: string, e: DragEvent): void;
}

// What's being dragged travels with the drag: types can be read while dragging (values only on
// drop), so a drag that never ended cleanly can't leave anything stale behind.
const ROW = "application/x-petramux-row"; // a sidebar row: one session (also DRAG_TYPE, for panes)
const SPACE = "application/x-petramux-space"; // a workspace's title line: the whole workspace
const FROM = "application/x-petramux-ws-";

function dragged(e: DragEvent): { kind: "row" | "space" | "pane"; ws: number | null } | null {
  const types = [...(e.dataTransfer?.types ?? [])];
  const from = types.find((t) => t.startsWith(FROM));
  const ws = from ? Number(from.slice(FROM.length)) : null;
  if (types.includes(SPACE)) return { kind: "space", ws };
  if (types.includes(ROW)) return { kind: "row", ws };
  if (types.includes(DRAG_TYPE)) return { kind: "pane", ws: null }; // a pane's header
  return null;
}

function unmark(el: HTMLElement) {
  el.querySelectorAll(".drop-before, .drop-after, .drop-swap, .drop-join").forEach((m) =>
    m.classList.remove("drop-before", "drop-after", "drop-swap", "drop-join"),
  );
}

/** One block per workspace, in order; the one on screen is lit, and several sessions share a card. */
export function render(
  el: HTMLElement,
  views: View[],
  groups: string[][],
  here: number,
  focusId: string | null,
  unread: Set<string>,
  windows: View[],
  on: Handlers,
) {
  el.replaceChildren();
  // let go on the sidebar's empty space: a session gets a workspace at the end, a workspace goes last
  const empty = (e: DragEvent) => !(e.target as Element).closest?.(".ws");
  el.ondragover = (e) => {
    if (dragged(e) && empty(e)) {
      e.preventDefault();
      unmark(el);
    }
  };
  el.ondragleave = (e) => {
    if (!el.contains(e.relatedTarget as Node | null)) unmark(el);
  };
  el.ondrop = (e) => {
    const d = dragged(e);
    unmark(el);
    if (!d || !empty(e)) return;
    e.preventDefault();
    const last = groups.at(-1)?.[0];
    if (d.kind === "space") {
      if (last) on.reorder(e.dataTransfer!.getData(SPACE), last, true);
    } else on.popOut(e.dataTransfer!.getData(DRAG_TYPE), null, true);
  };
  const byId = new Map(views.map((v) => [v.id, v]));
  groups.forEach((ids, ws) => {
    const block = document.createElement("div");
    const multi = ids.length > 1;
    block.className = `ws${ws === here ? " here" : ""}${multi ? " multi" : ""}`;
    if (multi) {
      const label = document.createElement("div");
      label.className = "wsh";
      label.textContent = `Workspace · ${ids.length}`;
      label.draggable = true;
      label.addEventListener("dragstart", (e) => {
        e.stopPropagation();
        e.dataTransfer?.setData(SPACE, ids[0]);
        e.dataTransfer?.setData(FROM + ws, "");
      });
      block.append(label);
    }
    const under = nested(ids, (id) => byId.get(id)?.parent);
    for (const id of ids) {
      const v = byId.get(id);
      if (v) block.append(row(v, ws, focusId, unread, on, under.has(id)));
    }
    // Onto a row of its own workspace, a row swaps places with it; onto the middle of another
    // workspace's row, a session joins that workspace; anything else lands before or after this
    // workspace (a session in a workspace of its own, a workspace moved).
    const target = (e: DragEvent) => {
      const d = dragged(e);
      if (!d) return null;
      const overRow = (e.target as Element).closest?.(".row") as HTMLElement | null;
      if (d.ws === ws) return d.kind === "row" && multi && overRow ? { d, swap: overRow, join: null } : null;
      const r = overRow?.getBoundingClientRect();
      const middle = r && e.clientY > r.top + r.height / 4 && e.clientY < r.bottom - r.height / 4;
      return { d, swap: null, join: middle && d.kind !== "space" ? overRow : null };
    };
    block.addEventListener("dragover", (e) => {
      const t = target(e);
      if (!t) return;
      e.preventDefault();
      unmark(el);
      if (t.swap) return t.swap.classList.add("drop-swap");
      if (t.join) return t.join.classList.add("drop-join");
      const r = block.getBoundingClientRect();
      block.classList.add(e.clientY > r.top + r.height / 2 ? "drop-after" : "drop-before");
    });
    block.addEventListener("drop", (e) => {
      const t = target(e);
      const after = block.classList.contains("drop-after");
      unmark(el);
      if (!t) return;
      e.preventDefault();
      if (t.d.kind === "space") on.reorder(e.dataTransfer!.getData(SPACE), ids[0], after);
      else if (t.swap) on.swap(e.dataTransfer!.getData(DRAG_TYPE), t.swap.dataset.id!);
      else if (t.join) on.join(e.dataTransfer!.getData(DRAG_TYPE), t.join.dataset.id!);
      else on.popOut(e.dataTransfer!.getData(DRAG_TYPE), ids[0], after);
    });
    if (block.querySelector(".row")) el.append(block);
  });
  if (windows.length) el.append(windowsBlock(windows, on));
}

/** Sessions in windows of their own: click one to bring its window to the front. */
function windowsBlock(windows: View[], on: Handlers) {
  const block = document.createElement("div");
  block.className = "windows";
  block.append(span("wsh", `Own windows · ${windows.length}`));
  for (const v of windows) {
    const row = document.createElement("div");
    row.className = `row s-${v.status}`;
    row.title = [v.name, v.note].filter(Boolean).join("\n");
    row.dataset.tipIfCut = "";
    const line = span("line", "");
    const meta = span("meta", v.status === "needs_you" ? "" : "⧉");
    if (v.status === "needs_you") meta.append(span("pill", "needs you"));
    line.append(span("dot", ""), span("name", v.name), meta);
    row.append(line);
    if (v.note) {
      const note = span("note", v.note);
      note.style.paddingLeft = "23px";
      row.append(note);
    }
    row.onclick = () => on.focusWindow(v.id);
    block.append(row);
  }
  return block;
}

function row(v: View, ws: number, focusId: string | null, unread: Set<string>, on: Handlers, child: boolean) {
  const row = document.createElement("div");
  const isUnread = unread.has(v.id);
  row.className = `row s-${v.status}${v.id === focusId ? " on" : ""}${isUnread ? " unread" : ""}`;
  row.dataset.id = v.id;
  row.title = [v.name, v.note].filter(Boolean).join("\n");
  row.dataset.tipIfCut = ""; // only when the row cuts something off (see tip.ts)
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
    note.style.paddingLeft = `${23 + (child ? 14 : 0)}px`;
    row.append(note);
  }
  row.onclick = () => on.select(v.id);
  row.ondblclick = () => {
    if (!document.body.classList.contains("rail")) startRename(name, v, on.rename);
  };
  row.draggable = true;
  row.addEventListener("dragstart", (e) => {
    e.dataTransfer?.setData(DRAG_TYPE, v.id); // panes take it too: dock it there
    e.dataTransfer?.setData(ROW, "");
    e.dataTransfer?.setData(FROM + ws, "");
  });
  row.addEventListener("dragend", (e) => on.dragEnd(v.id, e));
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
