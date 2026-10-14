import type { IBufferLine, Terminal } from "@xterm/xterm";
import { api } from "./api";

export interface PathMatch {
  start: number;
  end: number;
  path: string;
  line?: number;
  col?: number;
}

const CANDIDATE = /(?:~\/|\.{1,2}\/|\/)?[\w.@+-]+(?:\/[\w.@+-]+)*\/?(?::(\d+)(?::(\d+))?)?/g;

/** Words in a line of output that look like file paths (each still has to exist to be a link). */
export function findPaths(text: string): PathMatch[] {
  const out: PathMatch[] = [];
  for (const m of text.matchAll(CANDIDATE)) {
    const start = m.index;
    if (text[start - 1] === "/" || text[start - 1] === ":") continue; // part of a web or file URL
    const at = m[0].search(/:\d/);
    const path = (at < 0 ? m[0] : m[0].slice(0, at)).replace(/\.+$/, ""); // a sentence's full stop
    if (!path.includes("/") && !/\.[A-Za-z][\w-]{0,9}$/.test(path)) continue; // a word, a number
    if (/^\.*\/?$/.test(path)) continue;
    const end = at < 0 ? start + path.length : start + m[0].length;
    out.push({ start, end, path, line: m[1] ? Number(m[1]) : undefined, col: m[2] ? Number(m[2]) : undefined });
  }
  return out;
}

/** The line's text, and the cell each character sits in (wide characters take two cells). */
function cells(line: IBufferLine): { text: string; xs: number[] } {
  let text = "";
  const xs: number[] = [];
  for (let x = 0; x < line.length; x++) {
    const c = line.getCell(x);
    if (!c || c.getWidth() === 0) continue;
    const ch = c.getChars() || " ";
    for (let i = 0; i < ch.length; i++) xs.push(x);
    text += ch;
  }
  return { text, xs };
}

/** File paths in the output that exist, relative to the session's folder, ⌘-click to open. */
export function linkPaths(term: Terminal, base: () => string, failed: (e: unknown) => void) {
  term.registerLinkProvider({
    provideLinks(y, callback) {
      const line = term.buffer.active.getLine(y - 1);
      const { text, xs } = line ? cells(line) : { text: "", xs: [] };
      const found = findPaths(text);
      if (!found.length) return callback(undefined);
      const dir = base();
      api
        .existingPaths(dir, found.map((f) => f.path))
        .then((ok) =>
          callback(
            found
              .filter((_, i) => ok[i])
              .map((f) => ({
                range: { start: { x: xs[f.start] + 1, y }, end: { x: xs[f.end - 1] + 1, y } },
                text: text.slice(f.start, f.end),
                decorations: { underline: true, pointerCursor: true },
                activate: (e: MouseEvent) => {
                  if (e.metaKey) void api.openLink(f.path, dir, f.line, f.col).catch(failed);
                },
              })),
          ),
        )
        .catch(() => callback(undefined));
    },
  });
}
