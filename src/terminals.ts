import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import { api } from "./api";

interface Entry {
  term: Terminal;
  fit: FitAddon;
  el: HTMLDivElement;
}

const host = document.getElementById("terms")!;
const terms = new Map<string, Entry>();
let current: string | null = null;

const theme = {
  background: "#0f0f11",
  foreground: "#d6d6dc",
  cursor: "#f5a524",
  cursorAccent: "#0f0f11",
  selectionBackground: "#2a2a31",
};

function ensure(id: string): Entry {
  const existing = terms.get(id);
  if (existing) return existing;
  const el = document.createElement("div");
  el.className = "term";
  host.append(el);
  const term = new Terminal({
    fontFamily: '"JetBrains Mono", "SF Mono", Menlo, monospace',
    fontSize: 12.5,
    scrollback: 10000,
    cursorBlink: true,
    macOptionIsMeta: true,
    theme,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(el);
  try {
    term.loadAddon(new WebglAddon());
  } catch {
    // no WebGL: xterm keeps its DOM renderer
  }
  term.onData((data) => void api.write(id, data).catch(() => {}));
  // ⌘-keys belong to the app (and to the Edit menu for copy/paste), never to the PTY.
  term.attachCustomKeyEventHandler((ev) => !ev.metaKey);
  void api.attach(id, (text) => term.write(text));
  const entry = { term, fit, el };
  terms.set(id, entry);
  return entry;
}

function fitNow(id: string) {
  const e = terms.get(id);
  if (!e || e.el.hidden) return;
  e.fit.fit();
  void api.resize(id, e.term.cols, e.term.rows);
}

export function show(id: string) {
  const entry = ensure(id);
  for (const [key, e] of terms) e.el.hidden = key !== id;
  current = id;
  requestAnimationFrame(() => {
    fitNow(id);
    entry.term.focus();
  });
}

export function hideAll() {
  for (const e of terms.values()) e.el.hidden = true;
  current = null;
}

export function dispose(id: string) {
  const e = terms.get(id);
  if (!e) return;
  e.term.dispose();
  e.el.remove();
  terms.delete(id);
  if (current === id) current = null;
}

export function focusCurrent() {
  if (current) terms.get(current)?.term.focus();
}

new ResizeObserver(() => {
  if (current) fitNow(current);
}).observe(host);
