import { api } from "./api";

const basename = (p: string) => p.split("/").filter(Boolean).pop() ?? p;

/** Mirrors Rust `state::slug` so the sheet can preview the name; Rust stays authoritative. */
export function slug(prompt: string): string {
  const joined = prompt.trim().split(/\s+/).slice(0, 4).map((w) => w.toLowerCase()).join("-");
  let out = "";
  for (const c of joined) {
    if (/[a-z0-9]/.test(c)) out += c;
    else if (out && !out.endsWith("-")) out += "-";
  }
  out = out.slice(0, 24).replace(/-+$/, "");
  return out || "session";
}

export interface SheetOptions {
  repo: string;
  recentRepos: string[];
  start(prompt: string, repo: string, worktree: boolean): Promise<boolean>;
  closed(): void;
}

export function openSheet(o: SheetOptions) {
  let repo = o.repo;
  let git = false;
  let busy = false;
  const overlay = document.createElement("div");
  overlay.className = "overlay";
  overlay.innerHTML = `
    <div class="sheet">
      <textarea rows="3" placeholder="What should Claude do? (optional)"></textarea>
      <div class="sheet-row">
        <button class="chip repo"></button>
        <button class="chip wt">worktree <kbd>⌥↵</kbd></button>
        <button class="go">Start <kbd>↵</kbd></button>
      </div>
      <div class="sheet-row hint">
        <span>name: <b class="nm">session</b></span>
        <span>⇧↵ newline · esc cancel · ⌘P change repo</span>
      </div>
      <div class="repos" hidden></div>
    </div>`;
  document.body.append(overlay);
  const $ = <T extends HTMLElement>(sel: string) => overlay.querySelector<T>(sel)!;
  const ta = $<HTMLTextAreaElement>("textarea");
  const repoBtn = $<HTMLButtonElement>(".repo");
  const wt = $<HTMLButtonElement>(".wt");
  const repos = $<HTMLDivElement>(".repos");

  const close = () => {
    overlay.remove();
    o.closed();
  };

  const setRepo = async (path: string) => {
    repo = path;
    repoBtn.textContent = `▸ ${basename(path)}`;
    repoBtn.title = path;
    git = await api.isGitRepo(path);
    wt.classList.toggle("off", !git);
    wt.title = git ? "Start in a new git worktree" : "Not a git repo";
  };

  const start = async (worktree: boolean) => {
    if (busy || (worktree && !git)) return;
    busy = true;
    if (await o.start(ta.value, repo, worktree)) close();
    else busy = false;
  };

  const pick = (path: string) => {
    void setRepo(path);
    repos.hidden = true;
    ta.focus();
  };

  const item = (label: string, detail: string, onClick: () => void) => {
    const b = document.createElement("button");
    const l = document.createElement("span");
    const d = document.createElement("span");
    l.textContent = label;
    d.textContent = detail;
    d.className = "a";
    b.append(l, d);
    b.onclick = onClick;
    return b;
  };

  const toggleRepos = () => {
    repos.hidden = !repos.hidden;
    if (repos.hidden) return ta.focus();
    repos.replaceChildren(
      ...o.recentRepos.map((r) => item(basename(r), r, () => pick(r))),
      item("Open folder…", "", async () => {
        const p = await api.pickFolder();
        if (p) pick(p);
        else ta.focus();
      }),
    );
    (repos.firstElementChild as HTMLElement | null)?.focus();
  };

  ta.oninput = () => {
    $(".nm").textContent = slug(ta.value);
  };
  overlay.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      if (repos.hidden) close();
      else toggleRepos();
    } else if (e.metaKey && e.key.toLowerCase() === "p") {
      e.preventDefault();
      toggleRepos();
    } else if (e.key === "Enter" && !e.shiftKey && e.target === ta) {
      e.preventDefault();
      void start(e.altKey);
    }
  });
  overlay.onmousedown = (e) => {
    if (e.target === overlay) close();
  };
  $(".go").onclick = () => void start(false);
  wt.onclick = () => void start(true);
  repoBtn.onclick = toggleRepos;
  void setRepo(repo);
  ta.focus();
}
