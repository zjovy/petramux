import { expect, test } from "bun:test";
import { move, neighbor, panes, prune, remove, replace, setRatio, split, swap, zoneAt, type Node } from "./layout";

const p = (id: string): Node => ({ pane: id });
const row = (a: Node, b: Node, ratio = 0.5): Node => ({ split: "row", ratio, a, b });
const col = (a: Node, b: Node, ratio = 0.5): Node => ({ split: "col", ratio, a, b });

test("split puts the new pane on the requested side", () => {
  expect(split(null, "x", "a", "right")).toEqual(p("a"));
  expect(split(p("a"), "a", "b", "right")).toEqual(row(p("a"), p("b")));
  expect(split(p("a"), "a", "b", "left")).toEqual(row(p("b"), p("a")));
  expect(split(p("a"), "a", "b", "top")).toEqual(col(p("b"), p("a")));
  expect(split(p("a"), "a", "b", "bottom")).toEqual(col(p("a"), p("b")));
  expect(split(row(p("a"), p("b")), "b", "c", "bottom")).toEqual(row(p("a"), col(p("b"), p("c"))));
});

test("remove collapses the parent, and removing the last pane leaves null", () => {
  expect(remove(row(p("a"), col(p("b"), p("c"))), "b")).toEqual(row(p("a"), p("c")));
  expect(remove(p("a"), "a")).toBeNull();
});

test("prune drops unknown ids and repeats", () => {
  expect(prune(row(p("a"), row(p("gone"), p("a"))), new Set(["a"]))).toEqual(p("a"));
  expect(prune(null, new Set())).toBeNull();
});

test("panes lists ids in reading order", () => {
  expect(panes(row(p("a"), col(p("b"), p("c"))))).toEqual(["a", "b", "c"]);
});

test("replace and swap", () => {
  expect(replace(row(p("a"), p("b")), "b", "c")).toEqual(row(p("a"), p("c")));
  expect(swap(row(p("a"), col(p("b"), p("c"))), "a", "c")).toEqual(row(p("c"), col(p("b"), p("a"))));
});

test("move: edges split, centre swaps or replaces, onto itself is a no-op", () => {
  const t = row(p("a"), p("b"));
  expect(move(t, "a", "a", "left")).toBe(t);
  expect(move(t, "a", "b", "center")).toEqual(row(p("b"), p("a")));
  expect(move(t, "c", "b", "center")).toEqual(row(p("a"), p("c")));
  expect(move(t, "a", "b", "bottom")).toEqual(col(p("b"), p("a")));
  expect(move(t, "c", "a", "top")).toEqual(row(col(p("c"), p("a")), p("b")));
  expect(panes(move(t, "a", "b", "left")).sort()).toEqual(["a", "b"]);
});

test("setRatio follows the path and clamps", () => {
  const t = row(p("a"), col(p("b"), p("c")));
  expect(setRatio(t, [], 0.3)).toEqual(row(p("a"), col(p("b"), p("c")), 0.3));
  expect(setRatio(t, ["b"], 0.95)).toEqual(row(p("a"), col(p("b"), p("c"), 0.9)));
  expect(setRatio(t, ["b"], 0.01)).toEqual(row(p("a"), col(p("b"), p("c"), 0.1)));
});

test("zoneAt picks the nearest edge within 25%, else the centre", () => {
  const r = { left: 0, top: 0, width: 100, height: 100 };
  expect(zoneAt(r, 5, 50)).toBe("left");
  expect(zoneAt(r, 95, 50)).toBe("right");
  expect(zoneAt(r, 50, 5)).toBe("top");
  expect(zoneAt(r, 50, 95)).toBe("bottom");
  expect(zoneAt(r, 50, 50)).toBe("center");
  expect(zoneAt(r, 10, 4)).toBe("top");
});

test("neighbor finds the nearest pane in a direction", () => {
  const rects = {
    a: { left: 0, top: 0, width: 50, height: 100 },
    b: { left: 51, top: 0, width: 49, height: 50 },
    c: { left: 51, top: 51, width: 49, height: 49 },
  };
  expect(neighbor(rects, "a", "right")).toBe("b");
  expect(neighbor(rects, "b", "bottom")).toBe("c");
  expect(neighbor(rects, "c", "left")).toBe("a");
  expect(neighbor(rects, "a", "left")).toBeNull();
  expect(neighbor(rects, "nope", "left")).toBeNull();
});
