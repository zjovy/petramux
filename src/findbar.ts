import * as terms from "./terminals";

/** ⌘F: a find bar over a terminal. Enter goes up to older matches, ⇧Enter down, Esc closes. */
export function openFind(id: string, closed: () => void) {
  const host = terms.hostOf(id);
  if (!host) return;
  const open = host.querySelector<HTMLInputElement>(".find input");
  if (open) return void open.select();
  const bar = document.createElement("div");
  bar.className = "find";
  const input = document.createElement("input");
  input.placeholder = "Find";
  input.spellcheck = false;
  const count = document.createElement("span");
  count.className = "count";
  const go = (down: boolean, incremental = false) => terms.find(id, input.value, down, incremental);
  const close = () => {
    sub?.dispose();
    terms.endFind(id);
    bar.remove();
    closed();
  };
  const button = (label: string, title: string, act: () => void) => {
    const b = document.createElement("button");
    b.textContent = label;
    b.title = title;
    b.onmousedown = (e) => e.preventDefault(); // the input keeps the keyboard
    b.onclick = act;
    return b;
  };
  bar.append(input, count, button("↑", "Older match (Enter)", () => go(false)), button("↓", "Newer match (⇧Enter)", () => go(true)), button("✕", "Close (Esc)", close));
  const sub = terms.onFound(id, (i, n) => {
    count.textContent = n ? `${i < 0 ? "?" : i + 1}/${n}` : input.value ? "none" : "";
  });
  input.oninput = () => {
    if (!input.value) count.textContent = "";
    go(false, true);
  };
  input.onkeydown = (e) => {
    if (e.metaKey) return; // app shortcuts still work (⌘F selects the text again)
    e.stopPropagation(); // the terminal doesn't see these keys
    if (e.key === "Escape") close();
    else if (e.key === "Enter") {
      e.preventDefault();
      go(e.shiftKey);
    }
  };
  bar.onmousedown = (e) => e.stopPropagation(); // clicks here don't refocus the terminal
  host.append(bar);
  input.focus();
}
