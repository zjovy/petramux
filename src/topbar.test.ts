import { expect, test } from "bun:test";
import { counts, nextAfter, placeLabel } from "./topbar";

test("a folder reads as its name; a worktree as its repo and itself", () => {
  expect(placeLabel("/Users/me/Dev/siq")).toBe("siq");
  expect(placeLabel("/Users/me/Dev/siq/.claude/worktrees/app-dark-mode")).toBe("siq › app-dark-mode");
  expect(placeLabel("/Users/me/Dev/siq/.claude/worktrees/app-dark-mode/apps/web")).toBe("siq › app-dark-mode › apps/web");
  expect(placeLabel("/")).toBe("/");
});

test("what needs you and what's working, sessions and jobs alike", () => {
  const v = (status: string) => ({ status }) as never;
  expect(counts([v("needs_you"), v("working"), v("idle"), v("shell")], [v("needs_you"), v("working"), v("working")])).toEqual({
    needs: 2,
    working: 3,
  });
});

test("a chip goes to the next session in that state after the one you're on, round the list", () => {
  const order = ["a", "b", "c", "d"];
  const working = (id: string) => id === "a" || id === "c";
  expect(nextAfter(order, "a", working)).toBe("c");
  expect(nextAfter(order, "c", working)).toBe("a");
  expect(nextAfter(order, null, working)).toBe("a");
  expect(nextAfter(order, "b", () => false)).toBeUndefined();
});
