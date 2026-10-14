import { api, type Settings } from "./api";
import { terminalKeys, type Command } from "./shortcuts";
import * as themes from "./themes";

export type Section = "general" | "appearance" | "notifications" | "shortcuts";

export interface PrefsOptions {
  settings: Settings;
  commands: Command[];
  rail: boolean;
  setRail(on: boolean): void;
  /** Settings changed: save them and act on them. */
  change(s: Settings): void;
  openThemes(): void;
  closed(): void;
}

const SECTIONS: [Section, string][] = [
  ["general", "General"],
  ["appearance", "Appearance"],
  ["notifications", "Notifications"],
  ["shortcuts", "Keyboard Shortcuts"],
];

/** Mouse moves have no key to list them by, but they're half of how the app works. */
const MOUSE: { keys: string; label: string }[] = [
  { keys: "Drag a session", label: "Onto a pane's edge: split it in. Onto its middle: take its place" },
  { keys: "Drag a pane's header", label: "Onto another pane: move it. Onto the sidebar: its own workspace" },
  { keys: "Drag a sidebar row", label: "Out next to another workspace, or onto a row of its own workspace to swap" },
  { keys: "Drag a workspace's title", label: "Move the whole workspace" },
  { keys: "Double-click a name", label: "Rename the session" },
  { keys: "Drop files on a pane", label: "Paste their paths (Claude attaches images)" },
  { keys: "⌘-click a link or path", label: "Open it: web addresses in the browser, files in VS Code" },
];

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", text = ""): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text) e.textContent = text;
  return e;
}

/** Settings, as a sheet over the window (⌘, and the petramux menu); ⌘/ opens it at the shortcuts. */
export function openPrefs(o: PrefsOptions, start: Section = "general") {
  let s = { ...o.settings };
  const overlay = el("div", "overlay");
  const box = el("div", "prefs");
  const nav = el("div", "prefs-nav");
  const body = el("div", "prefs-body");
  box.append(nav, body);
  overlay.append(box);
  document.body.append(overlay);

  const save = (next: Settings) => {
    s = next;
    o.change(s);
  };
  const row = (label: string, hint: string, ...control: HTMLElement[]) => {
    const r = el("div", "pref");
    const text = el("div", "pref-text");
    text.append(el("div", "pref-label", label));
    if (hint) text.append(el("div", "pref-hint", hint));
    const c = el("div", "pref-control");
    c.append(...control);
    r.append(text, c);
    return r;
  };
  const toggle = (on: boolean, set: (on: boolean) => void) => {
    const b = el("input");
    b.type = "checkbox";
    b.checked = on;
    b.onchange = () => set(b.checked);
    return b;
  };
  const choice = (name: string, options: [string, string][], value: string, set: (v: string) => void) => {
    const wrap = el("div", "pref-choice");
    for (const [v, label] of options) {
      const l = el("label");
      const r = el("input");
      r.type = "radio";
      r.name = name;
      r.checked = v === value;
      r.onchange = () => set(v);
      l.append(r, document.createTextNode(` ${label}`));
      wrap.append(l);
    }
    return wrap;
  };
  const button = (label: string, act: () => void) => {
    const b = el("button", "btn", label);
    b.onclick = act;
    return b;
  };

  const sections: Record<Section, () => HTMLElement[]> = {
    general: () => {
      const folder = el("span", "pref-path", s.newSessionFolder ?? "");
      const where = choice(
        "where",
        [
          ["focused", "The focused session's folder"],
          ["fixed", "Always the same folder"],
        ],
        s.newSessionFolder ? "fixed" : "focused",
        async (v) => {
          if (v === "focused") return save({ ...s, newSessionFolder: null }), show("general");
          const p = s.newSessionFolder ?? (await api.pickFolder());
          save({ ...s, newSessionFolder: p ?? null });
          show("general");
        },
      );
      const pick = button("Choose…", async () => {
        const p = await api.pickFolder();
        if (p) save({ ...s, newSessionFolder: p });
        show("general");
      });
      return [
        row("New terminals open in", "⌘N and the + button.", where),
        ...(s.newSessionFolder ? [row("Folder", "", folder, pick)] : []),
        row(
          "When you quit",
          "Sessions that keep running are there, still working, when you reopen. ⌘W always ends one.",
          choice(
            "quit",
            [
              ["keep", "Keep sessions running"],
              ["stop", "Stop every session"],
            ],
            s.quitStopsSessions ? "stop" : "keep",
            (v) => save({ ...s, quitStopsSessions: v === "stop" }),
          ),
        ),
      ];
    },
    appearance: () => {
      const size = el("input", "pref-num");
      size.type = "number";
      size.step = "0.5";
      size.min = "6";
      size.max = "40";
      size.value = String(themes.fontSize());
      size.onchange = () => {
        const n = Number(size.value);
        if (n >= 6 && n <= 40) themes.setFontSize(n);
      };
      const font = el("input", "pref-text-input");
      font.placeholder = "JetBrains Mono (default)";
      font.value = themes.fontOverride();
      font.onchange = () => themes.setFontOverride(font.value);
      return [
        row("Theme", "Every Ghostty theme, previewed as you move through them.", el("span", "pref-value", themes.themeLabel()), button("Choose… ⌘⇧T", () => {
          close();
          o.openThemes();
        })),
        row("Font", "Any font installed on this Mac. Fonts set in your Ghostty config come after it.", font),
        row("Text size", "⌘= bigger, ⌘- smaller, ⌘0 back to your config's size.", size),
        row("Collapse the sidebar to a rail", "⌘B", toggle(o.rail, (on) => o.setRail(on))),
        row(
          "Claude uses the theme's colors",
          "With a theme on, Claude starts in its ANSI theme, light or dark to match. Running Claudes keep theirs until restarted.",
          toggle(s.claudeMatchesTheme, (on) => {
            save({ ...s, claudeMatchesTheme: on });
            themes.setClaudeMatching(on);
          }),
        ),
      ];
    },
    notifications: () => [
      row("When a session needs you", "A question, a permission, a plan to approve.", toggle(s.notifyNeedsYou, (on) => save({ ...s, notifyNeedsYou: on }))),
      row("When a session finishes", "Claude's turn is done.", toggle(s.notifyDone, (on) => save({ ...s, notifyDone: on }))),
      el("div", "pref-hint pref-note", "Never for the session you're looking at. macOS decides the rest: System Settings → Notifications → petramux."),
    ],
    shortcuts: () => {
      const groups = new Map<string, { keys: string; label: string }[]>();
      for (const c of o.commands) {
        if (!c.label) continue;
        groups.set(c.group, [...(groups.get(c.group) ?? []), c]);
      }
      groups.set("In the terminal", [...(groups.get("In the terminal") ?? []), ...terminalKeys()]);
      groups.set("With the mouse", MOUSE);
      const out: HTMLElement[] = [];
      for (const [group, list] of groups) {
        out.push(el("div", "keys-group", group));
        for (const k of list) {
          const r = el("div", "keys-row");
          r.append(el("kbd", "", k.keys), el("span", "", k.label));
          out.push(r);
        }
      }
      return out;
    },
  };

  const show = (which: Section) => {
    nav.replaceChildren(
      ...SECTIONS.map(([id, label]) => {
        const b = el("button", id === which ? "on" : "", label);
        b.onclick = () => show(id);
        return b;
      }),
    );
    body.replaceChildren(el("h2", "", SECTIONS.find(([id]) => id === which)![1]), ...sections[which]());
    body.scrollTop = 0;
  };

  const close = () => {
    overlay.remove();
    o.closed();
  };
  overlay.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") close();
  });
  overlay.onmousedown = (e) => {
    if (e.target === overlay) close();
  };
  overlay.tabIndex = -1;
  show(start);
  overlay.focus();
}
