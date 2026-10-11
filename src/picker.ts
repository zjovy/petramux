import { api, type Past } from "./api";
import { age } from "./sidebar";

const basename = (p: string) => p.split("/").filter(Boolean).pop() ?? p;

function div(cls: string, text: string) {
  const d = document.createElement("div");
  d.className = cls;
  d.textContent = text;
  return d;
}

export interface PickerOptions {
  repo: string;
  pick(p: Past): Promise<boolean>;
  closed(): void;
}

export async function openPicker(o: PickerOptions) {
  const overlay = document.createElement("div");
  overlay.className = "overlay";
  const box = div("picker", "");
  const input = document.createElement("input");
  input.placeholder = `Resume a session in ${basename(o.repo)}…`;
  const listEl = div("list", "");
  listEl.append(div("none", "Loading…"));
  box.append(input, listEl);
  overlay.append(box);
  document.body.append(overlay);
  input.focus();

  let all: Past[] = [];
  let shown: Past[] = [];
  let at = 0;
  let busy = false;

  const close = () => {
    overlay.remove();
    o.closed();
  };
  const mark = () => {
    listEl.querySelectorAll(".item").forEach((el, i) => el.classList.toggle("on", i === at));
    listEl.children[at]?.scrollIntoView({ block: "nearest" });
  };
  const choose = async (p: Past) => {
    if (busy) return;
    busy = true;
    if (await o.pick(p)) close();
    else busy = false;
  };
  const render = () => {
    const q = input.value.trim().toLowerCase();
    shown = all.filter((p) => p.title.toLowerCase().includes(q));
    at = Math.min(at, Math.max(0, shown.length - 1));
    listEl.replaceChildren();
    if (!shown.length) {
      listEl.append(div("none", all.length ? "No matches." : "No past sessions for this repo."));
      return;
    }
    shown.forEach((p, i) => {
      const row = document.createElement("button");
      row.className = `item${i === at ? " on" : ""}`;
      row.append(div("t", p.title), div("a", age(p.modifiedMs)));
      row.onmouseenter = () => {
        at = i;
        mark();
      };
      row.onclick = () => void choose(p);
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
    else if (e.key === "ArrowDown") {
      e.preventDefault();
      at = Math.min(at + 1, shown.length - 1);
      mark();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      at = Math.max(at - 1, 0);
      mark();
    } else if (e.key === "Enter" && shown[at]) {
      e.preventDefault();
      void choose(shown[at]);
    }
  });
  overlay.onmousedown = (e) => {
    if (e.target === overlay) close();
  };

  all = await api.listPast(o.repo).catch(() => []);
  render();
}
