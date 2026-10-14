/** One thing the palette can do. */
export interface Item {
  label: string;
  hint?: string;
  run(): void;
}

/** Items whose label and hint hold every word of the query. */
export function filter(items: Item[], query: string): Item[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return items.filter((it) => {
    const text = `${it.label} ${it.hint ?? ""}`.toLowerCase();
    return words.every((w) => text.includes(w));
  });
}

function div(cls: string, text: string) {
  const d = document.createElement("div");
  d.className = cls;
  d.textContent = text;
  return d;
}

/** ⌘⇧P: every command, session and background job, by typing part of its name. */
export function openPalette(items: Item[], closed: () => void) {
  const overlay = document.createElement("div");
  overlay.className = "overlay";
  const box = div("picker palette", "");
  const input = document.createElement("input");
  input.placeholder = "Type a command, a session or a job…";
  input.spellcheck = false;
  const listEl = div("list", "");
  box.append(input, listEl);
  overlay.append(box);
  document.body.append(overlay);
  input.focus();

  let shown: Item[] = [];
  let at = 0;
  const close = () => {
    overlay.remove();
    closed();
  };
  const choose = (it: Item) => {
    overlay.remove(); // gone before it runs: what it opens may take the keyboard
    it.run();
    if (document.activeElement === document.body || !document.activeElement) closed(); // nothing did
  };
  const mark = () => {
    listEl.querySelectorAll(".item").forEach((el, i) => el.classList.toggle("on", i === at));
    listEl.children[at]?.scrollIntoView({ block: "nearest" });
  };
  const render = () => {
    shown = filter(items, input.value);
    at = Math.min(at, Math.max(0, shown.length - 1));
    listEl.replaceChildren();
    if (!shown.length) return void listEl.append(div("none", "No matches."));
    shown.forEach((it, i) => {
      const row = document.createElement("button");
      row.className = `item${i === at ? " on" : ""}`;
      row.append(div("t", it.label), div("a", it.hint ?? ""));
      row.onmouseenter = () => {
        at = i;
        mark();
      };
      row.onclick = () => choose(it);
      listEl.append(row);
    });
  };
  input.oninput = () => {
    at = 0;
    render();
  };
  overlay.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") close();
    else if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
      e.preventDefault();
      at = Math.min(at + 1, shown.length - 1);
      mark();
    } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
      e.preventDefault();
      at = Math.max(at - 1, 0);
      mark();
    } else if (e.key === "Enter" && shown[at]) {
      e.preventDefault();
      choose(shown[at]);
    }
  });
  overlay.onmousedown = (e) => {
    if (e.target === overlay) close();
  };
  render();
}
