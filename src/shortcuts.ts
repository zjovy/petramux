import { ALT, META } from "./keys";

/** A ⌘ key: `shift`/`alt` must match exactly (`null`: either), `code` matches `KeyboardEvent.code`. */
export interface Chord {
  key: string | string[];
  shift?: boolean | null;
  alt?: boolean;
  code?: RegExp;
}

/** One app shortcut: how it's pressed, what the list calls it, and what it does. */
export interface Command {
  chord: Chord;
  keys: string;
  label: string;
  group: string;
  run(e: KeyboardEvent): void;
}

export function matches(c: Chord, e: Pick<KeyboardEvent, "key" | "code" | "shiftKey" | "altKey">): boolean {
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  const keyOk = c.code ? c.code.test(e.code) : [c.key].flat().some((k) => k === key || k === e.key);
  const shiftOk = c.shift === null || !!c.shift === e.shiftKey;
  return keyOk && shiftOk && !!c.alt === e.altKey;
}

const GLYPH: Record<string, string> = { Backspace: "⌫", Delete: "⌦", ArrowLeft: "←", ArrowRight: "→" };
const META_LABEL: Record<string, string> = {
  Backspace: "Delete to the start of the line",
  Delete: "Delete to the end of the line",
  ArrowLeft: "Start of the line",
  ArrowRight: "End of the line",
};
const ALT_LABEL: Record<string, string> = {
  Backspace: "Delete the word before",
  Delete: "Delete the word after",
  ArrowLeft: "Back a word",
  ArrowRight: "Forward a word",
};

/** The terminal's line-editing keys (keys.ts), for the list. */
export function terminalKeys(): { keys: string; label: string }[] {
  return [
    { keys: "⇧↩", label: "A new line, without sending" },
    ...Object.keys(META).map((k) => ({ keys: `⌘${GLYPH[k]}`, label: META_LABEL[k] })),
    ...Object.keys(ALT).map((k) => ({ keys: `⌥${GLYPH[k]}`, label: ALT_LABEL[k] })),
  ];
}
