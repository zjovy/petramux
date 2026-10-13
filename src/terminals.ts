import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import { api } from "./api";
import { editKey } from "./keys";
import { DEFAULT, fontStack, xtermTheme } from "./look";

interface Entry {
  term: Terminal;
  fit: FitAddon;
  el: HTMLDivElement;
}

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
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
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
  void api.attach(id, (text) => term.write(text));
  let timer: number | undefined;
  new ResizeObserver(() => {
    clearTimeout(timer);
    timer = window.setTimeout(() => refit(id), 50);
  }).observe(el);
  return { term, fit, el };
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

export function ids(): string[] {
  return [...terms.keys()];
}
