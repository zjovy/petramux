import { expect, test } from "bun:test";
import { findPaths } from "./links";

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
