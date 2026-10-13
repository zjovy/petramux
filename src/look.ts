import type { ITheme } from "@xterm/xterm";

/** Colors by their Ghostty key (`background`, `cursor-color`, `palette-3`, …). */
export type Colors = Record<string, string>;

/** Roost's own look, under any theme. */
export const DEFAULT: Colors = {
  background: "#0f0f11",
  foreground: "#d6d6dc",
  "cursor-color": "#f5a524",
  "cursor-text": "#0f0f11",
  "selection-background": "#2a2a31",
};

const ANSI = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
const BASE_FONTS = '"JetBrains Mono", "SF Mono", Menlo, monospace';

export function xtermTheme(c: Colors): ITheme {
  const t: Record<string, string | undefined> = {
    background: c.background,
    foreground: c.foreground,
    cursor: c["cursor-color"],
    cursorAccent: c["cursor-text"],
    selectionBackground: c["selection-background"],
    selectionForeground: c["selection-foreground"],
  };
  ANSI.forEach((name, i) => {
    t[name] = c[`palette-${i}`];
    t[`bright${name[0].toUpperCase()}${name.slice(1)}`] = c[`palette-${i + 8}`];
  });
  return Object.fromEntries(Object.entries(t).filter(([, v]) => v !== undefined));
}

/** Ghostty's `theme = light:A,dark:B` follows the system; a plain name is used either way. */
export function themeFor(spec: string, dark: boolean): string {
  const sides = new Map<string, string>();
  for (const part of spec.split(",")) {
    const m = part.trim().match(/^(light|dark):(.+)$/);
    if (m) sides.set(m[1], m[2].trim());
  }
  if (!sides.size) return spec.trim();
  return sides.get(dark ? "dark" : "light") ?? [...sides.values()][0];
}

/** Ghostty's `font-family` lines first, then Roost's fonts as fallbacks. */
export function fontStack(families: string[]): string {
  return [...families.map((f) => `"${f.replace(/"/g, "")}"`), BASE_FONTS].join(", ");
}
