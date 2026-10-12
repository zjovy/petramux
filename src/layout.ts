export type Side = "left" | "right" | "top" | "bottom";
export type Zone = Side | "center";
export type Node = { pane: string } | { split: "row" | "col"; ratio: number; a: Node; b: Node };
export type Tree = Node | null;
export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

const isPane = (n: Node): n is { pane: string } => "pane" in n;
const clamp = (r: number) => Math.min(0.9, Math.max(0.1, r));

export function panes(t: Tree): string[] {
  if (!t) return [];
  return isPane(t) ? [t.pane] : [...panes(t.a), ...panes(t.b)];
}

/** Drops panes whose id isn't in `keep`, and any repeat of an id already shown. */
export function prune(t: Tree, keep: Set<string>): Tree {
  const seen = new Set<string>();
  const walk = (n: Node): Tree => {
    if (isPane(n)) {
      if (!keep.has(n.pane) || seen.has(n.pane)) return null;
      seen.add(n.pane);
      return n;
    }
    const a = walk(n.a);
    const b = walk(n.b);
    if (!a) return b;
    if (!b) return a;
    return { ...n, a, b };
  };
  return t ? walk(t) : null;
}

export function remove(t: Tree, id: string): Tree {
  return prune(t, new Set(panes(t).filter((p) => p !== id)));
}

function mapPanes(t: Tree, f: (id: string) => Node): Tree {
  if (!t) return null;
  if (isPane(t)) return f(t.pane);
  return { ...t, a: mapPanes(t.a, f)!, b: mapPanes(t.b, f)! };
}

/** Puts `id` beside `target` on `side`; an empty tree becomes just `id`. */
export function split(t: Tree, target: string, id: string, side: Side): Tree {
  if (!t) return { pane: id };
  return mapPanes(t, (p) => {
    if (p !== target) return { pane: p };
    const dir = side === "left" || side === "right" ? "row" : "col";
    const first = side === "left" || side === "top";
    const fresh: Node = { pane: id };
    const old: Node = { pane: p };
    return { split: dir, ratio: 0.5, a: first ? fresh : old, b: first ? old : fresh };
  });
}

export function replace(t: Tree, oldId: string, newId: string): Tree {
  return mapPanes(t, (p) => ({ pane: p === oldId ? newId : p }));
}

export function swap(t: Tree, x: string, y: string): Tree {
  return mapPanes(t, (p) => ({ pane: p === x ? y : p === y ? x : p }));
}

/** Drag `id` (already in the tree, or from the sidebar) onto `target`'s `zone`. */
export function move(t: Tree, id: string, target: string, zone: Zone): Tree {
  if (id === target) return t;
  const inTree = panes(t).includes(id);
  if (zone === "center") return inTree ? swap(t, id, target) : replace(t, target, id);
  return split(inTree ? remove(t, id) : t, target, id, zone);
}

export function setRatio(t: Tree, path: ("a" | "b")[], ratio: number): Tree {
  if (!t || isPane(t)) return t;
  if (!path.length) return { ...t, ratio: clamp(ratio) };
  const [head, ...rest] = path;
  return { ...t, [head]: setRatio(t[head], rest, ratio) } as Node;
}

/** The nearest edge if the cursor is within 25% of it, otherwise the centre. */
export function zoneAt(r: Rect, x: number, y: number): Zone {
  const fx = (x - r.left) / r.width;
  const fy = (y - r.top) / r.height;
  const edges: [Side, number][] = [["left", fx], ["right", 1 - fx], ["top", fy], ["bottom", 1 - fy]];
  const [side, dist] = edges.reduce((m, c) => (c[1] < m[1] ? c : m));
  return dist < 0.25 ? side : "center";
}

/** Nearest pane in `dir` that overlaps `from` on the other axis: smallest gap, then centre offset. */
export function neighbor(rects: Record<string, Rect>, from: string, dir: Side): string | null {
  const f = rects[from];
  if (!f) return null;
  const cx = (r: Rect) => r.left + r.width / 2;
  const cy = (r: Rect) => r.top + r.height / 2;
  let best: [string, number] | null = null;
  for (const [id, r] of Object.entries(rects)) {
    if (id === from) continue;
    const horizontal = dir === "left" || dir === "right";
    const gap = horizontal
      ? dir === "right" ? r.left - (f.left + f.width) : f.left - (r.left + r.width)
      : dir === "bottom" ? r.top - (f.top + f.height) : f.top - (r.top + r.height);
    const overlap = horizontal
      ? r.top < f.top + f.height && f.top < r.top + r.height
      : r.left < f.left + f.width && f.left < r.left + r.width;
    if (gap < -1 || !overlap) continue;
    const score = gap * 1000 + (horizontal ? Math.abs(cy(r) - cy(f)) : Math.abs(cx(r) - cx(f)));
    if (!best || score < best[1]) best = [id, score];
  }
  return best ? best[0] : null;
}
