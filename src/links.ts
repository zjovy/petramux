import type { Terminal } from "@xterm/xterm";
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

/** What logicalLine reads of a buffer row (an xterm IBufferLine fits). */
interface Row {
  isWrapped: boolean;
  length: number;
  getCell(x: number): { getChars(): string; getWidth(): number } | undefined;
}

/** Row `y` with the rows it wraps into and out of, as one line of text, and the cell (row and
 *  column, 0-based) each character sits in (wide characters take two cells). */
export function logicalLine(get: (y: number) => Row | undefined, y: number): { text: string; at: { x: number; y: number }[] } {
  let first = y;
  while (first > 0 && get(first)?.isWrapped) first--;
  let text = "";
  const at: { x: number; y: number }[] = [];
  for (let r = first; r === first || get(r)?.isWrapped; r++) {
    const row = get(r);
    if (!row) break;
    for (let x = 0; x < row.length; x++) {
      const c = row.getCell(x);
      if (!c || c.getWidth() === 0) continue;
      const ch = c.getChars() || " ";
      for (let i = 0; i < ch.length; i++) at.push({ x, y: r });
      text += ch;
    }
  }
  return { text, at };
}

/** File paths in the output that exist, relative to the session's folder, ⌘-click to open. */
export function linkPaths(term: Terminal, base: () => string, failed: (e: unknown) => void) {
  term.registerLinkProvider({
    provideLinks(y, callback) {
      // the whole line this row is part of: a long path wraps onto the next row
      const { text, at } = logicalLine((r) => term.buffer.active.getLine(r), y - 1);
      const found = findPaths(text).filter((f) => at[f.start].y <= y - 1 && at[f.end - 1].y >= y - 1);
      if (!found.length) return callback(undefined);
      const dir = base();
      api
        .existingPaths(dir, found.map((f) => f.path))
        .then((ok) =>
          callback(
            found
              .filter((_, i) => ok[i])
              .map((f) => ({
                range: {
                  start: { x: at[f.start].x + 1, y: at[f.start].y + 1 },
                  end: { x: at[f.end - 1].x + 1, y: at[f.end - 1].y + 1 },
                },
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
