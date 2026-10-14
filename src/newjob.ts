import { placeLabel } from "./topbar";

const el = (tag: string, cls: string, text = "") => {
  const e = document.createElement(tag);
  e.className = cls;
  e.textContent = text;
  return e;
};

export interface NewJob {
  dir: string;
  pickFolder(): Promise<string | null>;
  start(prompt: string, dir: string): Promise<void>;
  closed(): void;
}

/** ⌘⇧N: a prompt for a new background job (`claude --bg`), in the focused session's folder. */
export function openNewJob(o: NewJob) {
  const overlay = el("div", "overlay");
  const box = el("div", "picker newjob");
  const input = document.createElement("textarea");
  input.placeholder = "What should Claude do in the background?";
  input.rows = 4;
  input.spellcheck = false;
  let dir = o.dir;
  const where = el("button", "where", "");
  const showDir = () => {
    where.textContent = `in ${dir ? placeLabel(dir) : "your home folder"}`;
    where.title = `${dir || "~"}\nClick to choose another folder`;
  };
  showDir();
  where.onclick = async () => {
    const picked = await o.pickFolder();
    if (picked) dir = picked;
    showDir();
    input.focus();
  };
  const error = el("div", "error", "");
  const start = el("button", "btn primary", "Start");
  const foot = el("div", "foot");
  foot.append(where, el("span", "hint", "↩ start · ⇧↩ new line · esc cancel"), start);
  box.append(input, error, foot);
  overlay.append(box);
  document.body.append(overlay);
  input.focus();

  let busy = false;
  const close = () => {
    overlay.remove();
    o.closed();
  };
  const submit = async () => {
    if (busy || !input.value.trim()) return;
    busy = true;
    start.textContent = "Starting…";
    try {
      await o.start(input.value, dir);
      close();
    } catch (e) {
      error.textContent = String(e);
      start.textContent = "Start";
      busy = false;
    }
  };
  start.onclick = () => void submit();
  overlay.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") close();
    else if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      void submit();
    }
  });
  overlay.onmousedown = (e) => {
    if (e.target === overlay) close();
  };
}
