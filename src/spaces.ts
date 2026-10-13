import { diffId } from "./diff";
import { panes, prune, remove, type Node, type Tree } from "./layout";

/**
 * Workspaces: each one is a saved arrangement of panes, and every session lives in exactly one.
 * Keeps each workspace's live panes (a session already shown in an earlier one is dropped),
 * drops emptied workspaces, and gives each session that has none a workspace of its own:
 * after its parent's (and its siblings'), else at the end.
 */
export function place(spaces: Tree[], keep: Set<string>, ids: string[], parentOf: (id: string) => string | null | undefined): Node[] {
  const seen = new Set<string>();
  const out: Node[] = [];
  for (const t of spaces) {
    const kept = prune(t, new Set([...keep].filter((id) => !seen.has(id))));
    if (!kept) continue;
    panes(kept).forEach((id) => seen.add(id));
    out.push(kept);
  }
  for (const id of ids) {
    if (seen.has(id)) continue;
    seen.add(id);
    const parent = parentOf(id);
    const kin = (t: Node) => panes(t).some((p) => p === parent || (parent != null && parentOf(p) === parent));
    let at = -1;
    if (parent) out.forEach((t, i) => kin(t) && (at = i));
    if (at >= 0) out.splice(at + 1, 0, { pane: id });
    else out.push({ pane: id });
  }
  return out;
}

/** Pulls a session (and its diff pane) out of every workspace but `keep`; an emptied one goes. */
export function without(spaces: Node[], id: string, keep: Tree): Node[] {
  return spaces.flatMap((t) => (t === keep ? [t] : [remove(remove(t, id), diffId(id))].filter((x): x is Node => x !== null)));
}

/** Moves item `from` just before or after item `to`. */
export function moveSpace<T>(list: T[], from: number, to: number, after: boolean): T[] {
  const out = list.slice();
  const [x] = out.splice(from, 1);
  out.splice(out.indexOf(list[to]) + (after ? 1 : 0), 0, x);
  return out;
}
