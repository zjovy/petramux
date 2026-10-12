/** Pure helpers for the diff pane: layout ids and `git diff` parsing. */
export const diffId = (id: string) => `diff:${id}`;
export const isDiff = (id: string) => id.startsWith("diff:");
export const owner = (id: string) => (isDiff(id) ? id.slice(5) : id);
/** Pane ids that may stay in the layout: each live session and its diff pane. */
export const keep = (ids: string[]) => new Set(ids.flatMap((id) => [id, diffId(id)]));

export interface DiffLine {
  kind: "hunk" | "ctx" | "add" | "del";
  text: string;
}

export interface DiffFile {
  path: string;
  add: number;
  del: number;
  binary: boolean;
  lines: DiffLine[];
}

/** `a/P b/P` names one unchanged path, even with spaces in it. */
function gitPath(header: string): string {
  const s = header.slice("diff --git ".length);
  const len = (s.length - 5) / 2;
  return Number.isInteger(len) && s.startsWith("a/") ? s.slice(2, 2 + len) : s;
}

export function parse(text: string): DiffFile[] {
  const files: DiffFile[] = [];
  let f: DiffFile | null = null;
  let inHunks = false;
  for (const raw of text.split("\n")) {
    if (raw.startsWith("diff --git ")) {
      f = { path: gitPath(raw), add: 0, del: 0, binary: false, lines: [] };
      files.push(f);
      inHunks = false;
    } else if (!f) continue;
    else if (raw.startsWith("@@")) {
      inHunks = true;
      f.lines.push({ kind: "hunk", text: raw });
    } else if (!inHunks) {
      if (raw.startsWith("--- a/")) f.path = raw.slice(6);
      else if (raw.startsWith("+++ b/")) f.path = raw.slice(6);
      else if (raw.startsWith("rename to ")) f.path = raw.slice(10);
      else if (raw.startsWith("Binary files ")) f.binary = true;
    } else if (raw.startsWith("+")) {
      f.add++;
      f.lines.push({ kind: "add", text: raw });
    } else if (raw.startsWith("-")) {
      f.del++;
      f.lines.push({ kind: "del", text: raw });
    } else if (raw.startsWith(" ")) f.lines.push({ kind: "ctx", text: raw });
  }
  return files;
}
