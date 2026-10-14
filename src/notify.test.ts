import { expect, test } from "bun:test";
import { backTo } from "./notify";

test("coming back soon after a notification goes to its session", () => {
  const n = { id: "s1", at: 1000, away: true };
  expect(backTo(n, 1000 + 5_000)).toBe("s1");
  expect(backTo(n, 1000 + 61_000)).toBeNull(); // long after: you came back for something else
  expect(backTo({ ...n, away: false }, 2000)).toBeNull(); // shown while you were in the app
  expect(backTo(null, 2000)).toBeNull();
});
