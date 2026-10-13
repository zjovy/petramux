import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/700.css";
import { api, type Appearance } from "./api";
import { DEFAULT, fontStack, themeFor, xtermTheme, type Colors } from "./look";
import * as terms from "./terminals";

const PICK = "roost.theme";
const ZOOM = "roost.zoom";
const dark = matchMedia("(prefers-color-scheme: dark)");
let config: Appearance = { theme: null, fontFamily: [], fontSize: null, colors: {} };
let shown: Colors = DEFAULT;
let seq = 0;
const ACCENTS = [
  ["--amber", "palette-3"],
  ["--blue", "palette-4"],
  ["--green", "palette-2"],
  ["--red", "palette-1"],
] as const;

function get(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null; // no storage: nothing remembered
  }
}

function put(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // no storage: the choice lasts until quit
  }
}

const configTheme = () => (config.theme ? themeFor(config.theme, dark.matches) : null);
const size = () => Math.min(40, Math.max(6, (config.fontSize ?? 12.5) + Number(get(ZOOM) ?? 0)));

/** Shows a theme: a picked one as it is, the config's with the config's own colors on top. */
async function apply(name: string | null, picked: boolean) {
  const mine = ++seq;
  const theme = name ? await api.theme(name).catch(() => null) : null;
  if (mine !== seq) return; // a newer choice is on its way
  shown = { ...(theme ?? DEFAULT), ...(picked ? {} : config.colors) };
  paint();
}

/** The window's chrome follows the terminal's background and text; the terminals take it all. */
function paint() {
  const root = document.documentElement.style;
  const bg = shown.background ?? DEFAULT.background;
  root.setProperty("--bg", bg);
  root.setProperty("--text", shown.foreground ?? DEFAULT.foreground);
  root.setProperty("--mono", fontStack(config.fontFamily));
  root.colorScheme = luminance(bg) < 0.5 ? "dark" : "light";
  // the status and diff accents come from the theme's palette too; without a theme, Roost's own stay
  for (const [prop, key] of ACCENTS) {
    if (shown[key]) root.setProperty(prop, shown[key]);
    else root.removeProperty(prop);
  }
  terms.setLook({ theme: xtermTheme(shown), fontFamily: fontStack(config.fontFamily), fontSize: size() });
}

function luminance(hex: string): number {
  const n = parseInt(hex.slice(1), 16);
  return (0.299 * (n >> 16) + 0.587 * ((n >> 8) & 255) + 0.114 * (n & 255)) / 255;
}

function refresh() {
  const picked = get(PICK);
  return apply(picked ?? configTheme(), picked !== null);
}

/** Reads the Ghostty config and shows the theme; edits to the config show when Roost is focused again. */
export async function init() {
  // Ghostty's (and so cmux's) default font ships with Roost; xterm measures cells once, so load it first.
  await Promise.all(["400", "700"].map((w) => document.fonts.load(`${w} 12px "JetBrains Mono"`).catch(() => [])));
  config = await api.appearance().catch(() => config);
  await refresh();
  dark.addEventListener("change", () => void refresh());
  window.addEventListener("focus", async () => {
    const next = await api.appearance().catch(() => config);
    if (JSON.stringify(next) === JSON.stringify(config)) return;
    config = next;
    await refresh();
  });
}

/** ⌘= / ⌘- grow and shrink the terminal font; ⌘0 goes back to the config's size. */
export function zoom(step: 1 | -1 | 0) {
  put(ZOOM, step === 0 ? null : String(Number(get(ZOOM) ?? 0) + step));
  paint();
}

function div(cls: string, text: string) {
  const d = document.createElement("div");
  d.className = cls;
  d.textContent = text;
  return d;
}

/** Every Ghostty theme, previewed live as you move; ↵ keeps one, esc puts back what was there. */
export async function openThemes(closed: () => void) {
  const fromConfig = configTheme();
  const items = [
    { label: fromConfig ? `Ghostty config · ${fromConfig}` : "petramux default", name: null as string | null },
    ...(await api.themes().catch(() => [] as string[])).map((n) => ({ label: n, name: n as string | null })),
  ];
  const overlay = div("overlay", "");
  const box = div("picker", "");
  const input = document.createElement("input");
  input.placeholder = "Theme…";
  const listEl = div("list", "");
  box.append(input, listEl);
  overlay.append(box);
  document.body.append(overlay);
  input.focus();

  let shownItems = items;
  let at = Math.max(0, items.findIndex((i) => i.name === get(PICK)));
  let done = false;
  const preview = () => {
    const it = shownItems[at];
    if (it) void apply(it.name ?? fromConfig, it.name !== null);
  };
  const close = (keep: boolean) => {
    if (done) return;
    done = true;
    if (keep && shownItems[at]) put(PICK, shownItems[at].name);
    else void refresh();
    overlay.remove();
    closed();
  };
  const mark = () => {
    listEl.querySelectorAll(".item").forEach((el, i) => el.classList.toggle("on", i === at));
    listEl.children[at]?.scrollIntoView({ block: "nearest" });
  };
  const render = () => {
    const q = input.value.trim().toLowerCase();
    shownItems = items.filter((i) => i.label.toLowerCase().includes(q));
    at = Math.min(at, Math.max(0, shownItems.length - 1));
    listEl.replaceChildren();
    if (!shownItems.length) return void listEl.append(div("none", "No matches."));
    shownItems.forEach((it, i) => {
      const row = document.createElement("button");
      row.className = `item${i === at ? " on" : ""}`;
      row.append(div("t", it.label));
      row.onmouseenter = () => {
        at = i;
        mark();
        preview();
      };
      row.onclick = () => close(true);
      listEl.append(row);
    });
    mark();
  };
  input.oninput = () => {
    at = 0;
    render();
    preview();
  };
  overlay.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") close(false);
    else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      at = Math.max(0, Math.min(shownItems.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)));
      mark();
      preview();
    } else if (e.key === "Enter") {
      e.preventDefault();
      close(true);
    }
  });
  overlay.onmousedown = (e) => {
    if (e.target === overlay) close(false);
  };
  render();
}
