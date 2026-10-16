import { expect, test } from "bun:test";
import { existsCache, findPaths, logicalLine } from "./links";

const paths = (text: string) => findPaths(text).map((m) => [text.slice(m.start, m.end), m.path, m.line, m.col]);

test("paths in a line of output, with a line and column when given", () => {
  expect(paths("⏺ Update(src/main.ts:12:5) and ./docs/a.md, ~/x/y.rs:3")).toEqual([
    ["src/main.ts:12:5", "src/main.ts", 12, 5],
    ["./docs/a.md", "./docs/a.md", undefined, undefined],
    ["~/x/y.rs:3", "~/x/y.rs", 3, undefined],
  ]);
  expect(paths("cat /Users/me/notes.txt.")).toEqual([["/Users/me/notes.txt", "/Users/me/notes.txt", undefined, undefined]]);
  expect(paths("Edited edited.txt")).toEqual([["edited.txt", "edited.txt", undefined, undefined]]);
});

test("words, numbers and web addresses aren't paths", () => {
  expect(paths("version 2.1.296 is fine, ok")).toEqual([]);
  expect(paths("see https://github.com/a/b.md and file:///tmp/x.txt")).toEqual([]);
});

/** Terminal rows as xterm keeps them: cells, and whether a row continues the one above. */
const rows = (...r: [string, boolean][]) => (i: number) => {
  const [text, isWrapped] = r[i] ?? [];
  if (text === undefined) return undefined;
  return {
    isWrapped,
    length: text.length,
    getCell: (x: number) => ({ getChars: () => text[x] ?? "", getWidth: () => 1 }),
  };
};

test("a path wrapped onto the next row is one line, with the cell of each character", () => {
  const get = rows(["see /Users/me/proj/sr", false], ["c/main.ts:3 ok", true], ["next", false]);
  for (const y of [0, 1]) {
    const line = logicalLine(get, y);
    expect(line.text).toBe("see /Users/me/proj/src/main.ts:3 ok");
    expect(line.at[4]).toEqual({ x: 4, y: 0 }); // the path starts on the first row
    expect(line.at[22]).toEqual({ x: 1, y: 1 }); // and goes on into the second
  }
  expect(logicalLine(get, 2).text).toBe("next");
});

test("which paths exist is remembered a while, so a redrawn line gets its links back at once", async () => {
  let asked = 0, t = 0;
  const exists = existsCache(async (_dir, paths) => (asked++, paths.map((p) => p === "a.ts")), 10_000, () => t);
  const first = exists("/r", ["a.ts", "b.ts"]);
  expect(first).toBeInstanceOf(Promise);
  expect(await first).toEqual([true, false]);
  expect(exists("/r", ["b.ts", "a.ts"])).toEqual([false, true]); // no waiting: answered from memory
  expect(exists("/other", ["a.ts"])).toBeInstanceOf(Promise); // another folder is another question
  t = 11_000;
  expect(exists("/r", ["a.ts"])).toBeInstanceOf(Promise); // old answers are asked again
  expect(asked).toBe(3); // the first, the other folder, the expired one
});
