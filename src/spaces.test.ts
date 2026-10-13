import { expect, test } from "bun:test";
import type { Node } from "./layout";
import { moveSpace, place, without } from "./spaces";

const p = (id: string): Node => ({ pane: id });
const row = (a: Node, b: Node): Node => ({ split: "row", ratio: 0.5, a, b });
const all = (...ids: string[]) => new Set(ids);
const noParent = () => null;

test("place keeps arrangements, drops closed sessions and the workspaces they emptied", () => {
  const spaces = [row(p("a"), p("b")), p("c"), p("d")];
  expect(place(spaces, all("a", "b", "d"), ["a", "b", "d"], noParent)).toEqual([row(p("a"), p("b")), p("d")]);
  expect(place(spaces, all("a", "c", "d"), ["a", "c", "d"], noParent)).toEqual([p("a"), p("c"), p("d")]);
});

test("a session with no workspace gets its own: at the end, or after its parent's", () => {
  const spaces = [row(p("a"), p("b")), p("c")];
  const parentOf = (id: string) => (id === "k1" || id === "k2" ? "a" : null);
  expect(place(spaces, all("a", "b", "c", "n"), ["a", "b", "c", "n"], parentOf)).toEqual([row(p("a"), p("b")), p("c"), p("n")]);
  expect(place(spaces, all("a", "b", "c", "k1", "k2"), ["a", "b", "c", "k1", "k2"], parentOf)).toEqual([
    row(p("a"), p("b")),
    p("k1"),
    p("k2"),
    p("c"),
  ]);
});

test("a session never shows in two workspaces: the first keeps it", () => {
  expect(place([row(p("a"), p("b")), p("b")], all("a", "b"), ["a", "b"], noParent)).toEqual([row(p("a"), p("b"))]);
});

test("a session left out of the kept set still gets a workspace of its own", () => {
  expect(place([row(p("a"), p("x"))], all("a"), ["a", "x"], noParent)).toEqual([p("a"), p("x")]);
});

test("without pulls a session and its diff out of every other workspace", () => {
  const here = row(p("a"), p("b"));
  const spaces = [p("c"), here, row(p("c"), p("diff:c")), p("d")];
  expect(without(spaces, "c", here)).toEqual([here, p("d")]);
  expect(without(spaces, "c", here)[0]).toBe(here);
});

test("moveSpace puts a workspace before or after another", () => {
  expect(moveSpace(["a", "b", "c", "d"], 3, 0, false)).toEqual(["d", "a", "b", "c"]);
  expect(moveSpace(["a", "b", "c", "d"], 0, 2, true)).toEqual(["b", "c", "a", "d"]);
  expect(moveSpace(["a", "b", "c", "d"], 1, 3, true)).toEqual(["a", "c", "d", "b"]);
});
