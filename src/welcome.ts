/**
 * The welcome sheet: what petramux can do, in four cards, each a small moving sketch of the
 * gesture and the keys that go with it. Opens once on the first launch; ⌘⇧P → Welcome brings it back.
 */

const SEEN = "roost.welcomed";

export const welcomed = () => {
  try {
    return localStorage.getItem(SEEN) === "1";
  } catch {
    return true; // no storage: don't show it on every launch
  }
};

interface Card {
  title: string;
  text: string;
  sketch: string;
  keys: [string, string][];
}

const row = (cls = "") => `<i class="w-row ${cls}"><b></b><s></s></i>`;
const pane = (cls = "") => `<i class="w-pane ${cls}"><b></b><s></s><s></s></i>`;

const CARDS: Card[] = [
  {
    title: "Sessions",
    text: "Every terminal and every Claude gets a row. A row lights up when Claude needs you.",
    sketch: `<div class="w-side">${row()}${row("w-needs")}${row("w-new")}</div>`,
    keys: [
      ["⌘N", "New terminal"],
      ["⌘J", "Next one that needs you"],
      ["⌘⇧F", "Fork the conversation"],
    ],
  },
  {
    title: "Workspaces",
    text: "Drag a session's row onto another's to put them side by side. Each workspace keeps its own layout.",
    sketch: `<div class="w-side">${row("w-a")}${row("w-drag")}</div><div class="w-main w-join">${pane()}${pane("w-joined")}</div>`,
    keys: [
      ["⌘D", "Split with a new terminal"],
      ["⌘1…9", "Go to a workspace"],
    ],
  },
  {
    title: "Panes",
    text: "Drag a pane by its header onto another pane to move it there, or out of the window for a window of its own. Drop files in to paste their paths.",
    sketch: `<div class="w-main w-move">${pane("w-left")}${pane("w-right")}</div>`,
    keys: [
      ["⌘⇧↩", "Zoom a pane, or back"],
      ["⌘⌥←→↑↓", "Focus the pane that way"],
      ["⌘G", "What Claude changed"],
    ],
  },
  {
    title: "Find anything",
    text: "Search every session and command from one box. The sidebar folds to a rail of dots when you need the room.",
    sketch: `<div class="w-search"><b></b><i class="w-hit"></i><i></i><i></i></div>`,
    keys: [
      ["⌘⇧P", "Search"],
      ["⌘/", "Every shortcut"],
      ["⌘B", "Collapse the sidebar"],
    ],
  },
];

export function openWelcome(o: { shortcuts: () => void; closed: () => void }) {
  try {
    localStorage.setItem(SEEN, "1");
  } catch {
    // no storage: it may open again next launch
  }
  const overlay = document.createElement("div");
  overlay.className = "overlay";
  overlay.tabIndex = -1;
  const box = document.createElement("div");
  box.className = "welcome";
  box.innerHTML = `<h2>Welcome to petramux</h2><p class="w-lede">Claude Code sessions, side by side. A minute's worth of what's here:</p>`;
  const grid = document.createElement("div");
  grid.className = "w-grid";
  for (const c of CARDS) {
    const card = document.createElement("section");
    card.className = "w-card";
    const sketch = document.createElement("div");
    sketch.className = "w-sketch";
    sketch.setAttribute("aria-hidden", "true");
    sketch.innerHTML = c.sketch;
    const title = document.createElement("h3");
    title.textContent = c.title;
    const text = document.createElement("p");
    text.textContent = c.text;
    const keys = document.createElement("dl");
    for (const [k, label] of c.keys) {
      const dt = document.createElement("dt");
      dt.append(Object.assign(document.createElement("kbd"), { textContent: k }));
      const dd = document.createElement("dd");
      dd.textContent = label;
      keys.append(dt, dd);
    }
    card.append(sketch, title, text, keys);
    grid.append(card);
  }
  const foot = document.createElement("div");
  foot.className = "w-foot";
  const all = Object.assign(document.createElement("button"), { className: "btn", textContent: "See all shortcuts" });
  const ok = Object.assign(document.createElement("button"), { className: "btn primary", textContent: "Got it" });
  foot.append(all, ok);
  box.append(grid, foot);
  overlay.append(box);
  document.body.append(overlay);

  const close = () => {
    overlay.remove();
    o.closed();
  };
  ok.onclick = close;
  all.onclick = () => {
    overlay.remove();
    o.shortcuts();
  };
  overlay.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") close();
  });
  overlay.onmousedown = (e) => {
    if (e.target === overlay) close();
  };
  ok.focus();
}
