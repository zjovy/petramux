import { expect, test } from "bun:test";
import { editKey } from "./keys";

const k = (key: string, mods: Partial<Record<"metaKey" | "altKey" | "ctrlKey" | "shiftKey", boolean>> = {}) =>
  editKey({ key, metaKey: false, altKey: false, ctrlKey: false, shiftKey: false, ...mods });

test("⌘ keys edit and move by line", () => {
  expect(k("Backspace", { metaKey: true })).toBe("\x15");
  expect(k("Delete", { metaKey: true })).toBe("\x0b");
  expect(k("ArrowLeft", { metaKey: true })).toBe("\x01");
  expect(k("ArrowRight", { metaKey: true })).toBe("\x05");
});

test("⌥ keys edit and move by word", () => {
  expect(k("Backspace", { altKey: true })).toBe("\x1b\x7f");
  expect(k("Delete", { altKey: true })).toBe("\x1bd");
  expect(k("ArrowLeft", { altKey: true })).toBe("\x1bb");
  expect(k("ArrowRight", { altKey: true })).toBe("\x1bf");
});

test("other keys and chords are left alone", () => {
  expect(k("Backspace")).toBeUndefined();
  expect(k("ArrowLeft", { metaKey: true, altKey: true })).toBeUndefined(); // ⌘⌥← moves pane focus
  expect(k("ArrowLeft", { metaKey: true, shiftKey: true })).toBeUndefined();
  expect(k("a", { metaKey: true })).toBeUndefined();
});

test("⇧↩ is a new line, not Enter (as Option-↩ is: Claude adds a line, a shell too)", () => {
  expect(k("Enter", { shiftKey: true })).toBe("\x1b\r");
  expect(k("Enter")).toBeUndefined();
  expect(k("Enter", { shiftKey: true, metaKey: true })).toBeUndefined(); // ⌘⇧↩ zooms the pane
});
