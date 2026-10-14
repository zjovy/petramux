/** macOS line editing in a terminal (Ghostty's defaults), as bytes for the shell or Claude. */
export const META: Record<string, string> = { Backspace: "\x15", Delete: "\x0b", ArrowLeft: "\x01", ArrowRight: "\x05" };
export const ALT: Record<string, string> = { Backspace: "\x1b\x7f", Delete: "\x1bd", ArrowLeft: "\x1bb", ArrowRight: "\x1bf" };

export function editKey(e: Pick<KeyboardEvent, "key" | "metaKey" | "altKey" | "ctrlKey" | "shiftKey">): string | undefined {
  // ⇧↩: a new line in what you're typing, as Option-↩ gives (xterm would send a plain Enter)
  if (e.key === "Enter" && e.shiftKey && !e.metaKey && !e.altKey && !e.ctrlKey) return "\x1b\r";
  if (e.ctrlKey || e.shiftKey || e.metaKey === e.altKey) return undefined; // exactly one of ⌘ or ⌥
  return (e.metaKey ? META : ALT)[e.key];
}
