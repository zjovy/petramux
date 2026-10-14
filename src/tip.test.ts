import { expect, test } from "bun:test";
import { place } from "./tip";

test("a tip sits below and right of the pointer", () => {
  expect(place(100, 100, 120, 30, 800, 600)).toEqual({ left: 110, top: 118 });
});

test("a tip stays inside the window", () => {
  // near the right edge: pulled left
  expect(place(780, 100, 120, 30, 800, 600).left).toBe(800 - 120 - 6);
  // near the bottom: above the pointer
  expect(place(100, 590, 120, 30, 800, 600).top).toBe(590 - 30 - 8);
});
