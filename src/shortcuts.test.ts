import { expect, test } from "bun:test";
import { matches, terminalKeys, type Chord } from "./shortcuts";

const ev = (key: string, mods: { shift?: boolean; alt?: boolean; code?: string } = {}) =>
  ({ key, code: mods.code ?? "", metaKey: true, shiftKey: !!mods.shift, altKey: !!mods.alt }) as KeyboardEvent;

test("a chord matches its key and exactly its modifiers", () => {
  const split: Chord = { key: "d" };
  const down: Chord = { key: "d", shift: true };
  expect(matches(split, ev("d"))).toBe(true);
  expect(matches(split, ev("D", { shift: true }))).toBe(false);
  expect(matches(down, ev("D", { shift: true }))).toBe(true);
  expect(matches({ key: "ArrowLeft", alt: true }, ev("ArrowLeft", { alt: true }))).toBe(true);
  expect(matches({ key: "ArrowLeft", alt: true }, ev("ArrowLeft"))).toBe(false);
  expect(matches({ key: ["=", "+"] }, ev("+"))).toBe(true);
});

test("the terminal's own editing keys are listed from the same table they run from", () => {
  const keys = terminalKeys().map((k) => k.keys);
  expect(keys).toContain("⌘⌫");
  expect(keys).toContain("⌥←");
});

test("a chord by the key's place matches what ⌥ turns into a dead key", () => {
  const optN = { key: "Dead", code: "KeyN", shiftKey: false, altKey: true };
  expect(matches({ key: "", alt: true, code: /^KeyN$/ }, optN)).toBe(true);
  expect(matches({ key: "", alt: true, code: /^KeyN$/ }, { ...optN, altKey: false })).toBe(false);
});
