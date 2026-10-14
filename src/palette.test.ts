import { expect, test } from "bun:test";
import { filter } from "./palette";

const items = ["New terminal", "Split right with a new terminal", "Go to trd", "Choose a theme"].map((label) => ({ label, run() {} }));
const labels = (q: string) => filter(items, q).map((i) => i.label);

test("the palette keeps items holding every typed word", () => {
  expect(labels("")).toHaveLength(4);
  expect(labels("term")).toEqual(["New terminal", "Split right with a new terminal"]);
  expect(labels("split TERM")).toEqual(["Split right with a new terminal"]);
  expect(labels("go trd")).toEqual(["Go to trd"]);
  expect(labels("nothing")).toEqual([]);
});
