import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon, type ISearchOptions } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import { api } from "./api";
import { editKey } from "./keys";
import { linkPaths } from "./links";
import { DEFAULT, fontStack, xtermTheme } from "./look";

interface Entry {
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  el: HTMLDivElement;
}

/** The folder a session's relative paths are in, and where a failed open is reported. */
let folderOf: (id: string) => string = () => "";
let failed: (e: unknown) => void = console.error;
export function setLinks(folder: (id: string) => string, report: (e: unknown) => void) {
  folderOf = folder;
  failed = report;
}

/** ⌘-click: a web address, a file URL (OSC 8) or a path opens; a plain click stays a click. */
const open = (id: string) => (e: MouseEvent, uri: string) => {
  if (e.metaKey) void api.openLink(uri, folderOf(id)).catch(failed);
};

const terms = new Map<string, Entry>();
// Terminals not in any pane live here, still running and still buffering output.
const parking = document.createElement("div");
parking.hidden = true;
document.body.append(parking);

export interface Look {
  theme: ITheme;
  fontFamily: string;
  fontSize: number;
}
let look: Look = { theme: xtermTheme(DEFAULT), fontFamily: fontStack([]), fontSize: 12.5 };

/** New colors or fonts for every terminal, open or parked. */
export function setLook(l: Look) {
  look = l;
  for (const [id, e] of terms) {
    Object.assign(e.term.options, l);
    refit(id);
  }
}

function create(id: string, parent: HTMLElement): Entry {
  const el = document.createElement("div");
  el.className = "term-host";
  parent.append(el); // open in a visible box so xterm can measure
  const term = new Terminal({
    ...look,
    scrollback: 10000,
    cursorBlink: true,
    macOptionIsMeta: true,
    allowProposedApi: true, // search highlights
    linkHandler: { activate: open(id), allowNonHttpProtocols: true },
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  const search = new SearchAddon();
  term.loadAddon(search);
  term.loadAddon(new WebLinksAddon(open(id)));
  linkPaths(term, () => folderOf(id), (e) => failed(e));
  term.open(el);
  try {
    const gl = new WebglAddon();
    // WebKit keeps ~16 live WebGL contexts and drops the oldest; fall back to the DOM renderer.
    gl.onContextLoss(() => gl.dispose());
    term.loadAddon(gl);
  } catch {
    // no WebGL: xterm keeps its DOM renderer
  }
  term.onData((data) => void api.write(id, data).catch(() => {}));
  // ⌘/⌥ editing keys become line edits; other ⌘-keys belong to the app (and the Edit menu), never the PTY.
  term.attachCustomKeyEventHandler((ev) => {
    const bytes = ev.type === "keydown" ? editKey(ev) : undefined;
    if (bytes) {
      ev.preventDefault();
      void api.write(id, bytes).catch(() => {});
      return false;
    }
    return !ev.metaKey;
  });
  // Sized to its pane before anything is written: a session kept running replays its screen
  // here, and drawn at xterm's default 80 columns it would come out garbled.
  if (el.clientWidth > 0) {
    fit.fit();
    void api.resize(id, term.cols, term.rows).catch(() => {});
  }
  void api.attach(id, (text) => term.write(text));
  let timer: number | undefined;
  new ResizeObserver(() => {
    clearTimeout(timer);
    timer = window.setTimeout(() => refit(id), 50);
  }).observe(el);
  return { term, fit, search, el };
}

/** Mixes two #rrggbb colors: `t` of `a`, the rest `b`. */
function mix(a: string, b: string, t: number): string {
  const ch = (h: string, i: number) => parseInt(h.slice(1 + 2 * i, 3 + 2 * i), 16);
  const ok = (h: string) => /^#[0-9a-f]{6}$/i.test(h);
  if (!ok(a) || !ok(b)) return a;
  return "#" + [0, 1, 2].map((i) => Math.round(ch(a, i) * t + ch(b, i) * (1 - t)).toString(16).padStart(2, "0")).join("");
}

/** Finds `query` in the terminal's scrollback, upward (older) by default; matches are lit. */
export function find(id: string, query: string, down = false, incremental = false): void {
  const e = terms.get(id);
  if (!e) return;
  if (!query) return e.search.clearDecorations();
  const amber = "#f5a524";
  const bg = look.theme.background ?? "#000000";
  const opts: ISearchOptions = {
    incremental,
    decorations: {
      matchBackground: mix(amber, bg, 0.3),
      matchOverviewRuler: amber,
      activeMatchBackground: mix(amber, bg, 0.7),
      activeMatchBorder: amber,
      activeMatchColorOverviewRuler: amber,
    },
  };
  if (down) e.search.findNext(query, opts);
  else e.search.findPrevious(query, opts);
}

/** Called with "match n of count" as the matches change (count 0: none). */
export function onFound(id: string, f: (index: number, count: number) => void) {
  return terms.get(id)?.search.onDidChangeResults((r) => f(r.resultIndex, r.resultCount));
}

export function endFind(id: string) {
  const e = terms.get(id);
  e?.search.clearDecorations();
  e?.term.clearSelection();
}

/** ⌘K: clears the screen and scrollback, as Terminal does (the program redraws what it shows). */
export function clear(id: string) {
  terms.get(id)?.term.clear();
}

export function mount(id: string, parent: HTMLElement) {
  const e = terms.get(id);
  if (!e) terms.set(id, create(id, parent));
  else if (e.el.parentElement !== parent) parent.append(e.el);
}

export function park(id: string) {
  const e = terms.get(id);
  if (e && e.el.parentElement !== parking) parking.append(e.el);
}

export function refit(id: string) {
  const e = terms.get(id);
  if (!e || e.el.parentElement === parking || e.el.clientWidth === 0) return;
  e.fit.fit();
  void api.resize(id, e.term.cols, e.term.rows);
}

/** Types text the way a paste does (bracketed, if the program asked for that). */
export function paste(id: string, text: string) {
  terms.get(id)?.term.paste(text);
}

export function focus(id: string) {
  terms.get(id)?.term.focus();
}

export function dispose(id: string) {
  const e = terms.get(id);
  if (!e) return;
  e.term.dispose();
  e.el.remove();
  terms.delete(id);
}

/** The box a terminal draws in: it moves with the terminal from pane to pane. */
export function hostOf(id: string): HTMLElement | undefined {
  return terms.get(id)?.el;
}

export function ids(): string[] {
  return [...terms.keys()];
}
