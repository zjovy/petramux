/** `file://` lines of a `text/uri-list` as paths (Finder gives these when the webview passes them on). */
export function pathsFromUris(list: string): string[] {
  return list
    .split(/\r?\n/)
    .filter((l) => l.startsWith("file://"))
    .map((l) => decodeURIComponent(new URL(l).pathname));
}

/** Backslash-escapes a path as Terminal does when a file is dropped on it. */
export function escapePath(p: string): string {
  return p.replace(/[^\w@%+=:,./-]/g, "\\$&");
}

/** A dropped file's name, reduced to characters that are safe in a header and a file name. */
export function safeName(name: string): string {
  return name.replace(/[^\w.-]+/g, "_") || "file";
}
