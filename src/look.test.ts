import { expect, test } from "bun:test";
import { fontStack, themeFor, xtermTheme } from "./look";

test("Ghostty colors map onto xterm's theme", () => {
  const t = xtermTheme({
    background: "#1e1e2e",
    foreground: "#cdd6f4",
    "cursor-color": "#f5e0dc",
    "cursor-text": "#111111",
    "selection-background": "#585b70",
    "palette-1": "#f38ba8",
    "palette-9": "#f37799",
    "palette-15": "#bac2de",
  });
  expect(t).toEqual({
    background: "#1e1e2e",
    foreground: "#cdd6f4",
    cursor: "#f5e0dc",
    cursorAccent: "#111111",
    selectionBackground: "#585b70",
    red: "#f38ba8",
    brightRed: "#f37799",
    brightWhite: "#bac2de",
  });
});

test("a light:/dark: theme follows the system; a plain one is used either way", () => {
  expect(themeFor("light:Catppuccin Latte,dark:Catppuccin Mocha", true)).toBe("Catppuccin Mocha");
  expect(themeFor("dark:Catppuccin Mocha, light:Catppuccin Latte", false)).toBe("Catppuccin Latte");
  expect(themeFor("Dracula", false)).toBe("Dracula");
  expect(themeFor("dark:Dracula", false)).toBe("Dracula");
});

test("Ghostty's fonts lead the stack and Roost's defaults catch the rest", () => {
  expect(fontStack(["Berkeley Mono", "Symbols Nerd Font"])).toBe('"Berkeley Mono", "Symbols Nerd Font", "JetBrains Mono", "SF Mono", Menlo, monospace');
  expect(fontStack([])).toBe('"JetBrains Mono", "SF Mono", Menlo, monospace');
});
