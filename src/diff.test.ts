import { expect, test } from "bun:test";
import { diffId, isDiff, keep, owner, parse } from "./diff";

const sample = [
  "diff --git a/src/a b.ts b/src/a b.ts", "index 1..2 100644", "--- a/src/a b.ts", "+++ b/src/a b.ts",
  "@@ -1,2 +1,3 @@", " keep", "-old", "+new", "+++ looks like a header", "\\ No newline at end of file",
  "diff --git a/img.png b/img.png", "new file mode 100644", "index 0000000..1", "Binary files /dev/null and b/img.png differ",
  "diff --git a/gone.txt b/gone.txt", "deleted file mode 100644", "--- a/gone.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-bye",
  "diff --git a/old.ts b/new.ts", "similarity index 100%", "rename from old.ts", "rename to new.ts", "",
].join("\n");

test("files get their paths and counts", () => {
  const files = parse(sample);
  expect(files.map((f) => f.path)).toEqual(["src/a b.ts", "img.png", "gone.txt", "new.ts"]);
  expect([files[0].add, files[0].del]).toEqual([2, 1]);
  expect(files[1].binary).toBe(true);
  expect([files[2].add, files[2].del]).toEqual([0, 1]);
});

test("hunk lines keep their kind and text, and +++ inside a hunk is an added line", () => {
  expect(parse(sample)[0].lines.map((l) => l.kind)).toEqual(["hunk", "ctx", "del", "add", "add"]);
  expect(parse(sample)[0].lines[4].text).toBe("+++ looks like a header");
});

test("an empty diff has no files", () => expect(parse("")).toEqual([]));

test("diff panes are tied to their session", () => {
  expect(diffId("s1")).toBe("diff:s1");
  expect(isDiff("diff:s1") && !isDiff("s1")).toBe(true);
  expect([owner("diff:s1"), owner("s1")]).toEqual(["s1", "s1"]);
  expect([...keep(["s1"])]).toEqual(["s1", "diff:s1"]);
});
