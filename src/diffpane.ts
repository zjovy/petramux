import { api, type Diff, type View } from "./api";
import { owner, parse, truncated } from "./diff";

type Scope = "turn" | "all";
const scopes = new Map<string, Scope>(); // per diff pane, for this run of the app
const seen = new Map<string, string>(); // last status seen per diff pane: a change re-reads the diff
let req = 0;

const el = (tag: string, cls: string, text = "") => {
  const e = document.createElement(tag);
  e.className = cls;
  e.textContent = text;
  return e;
};

/** A rebuilt pane has an empty body: make its next `update` load. */
export const forget = (paneId: string) => seen.delete(paneId);

function message(body: HTMLElement, text: string) {
  body.replaceChildren(el("div", "diff-msg", text));
}

async function load(body: HTMLElement, paneId: string) {
  const mine = String(++req);
  body.dataset.req = mine;
  const scope = scopes.get(paneId) ?? "turn";
  let d: Diff;
  try {
    d = await api.diff(owner(paneId), scope);
  } catch (e) {
    if (body.dataset.req === mine) message(body, String(e));
    return;
  }
  if (body.dataset.req !== mine) return; // a newer load is on its way
  const text = d.text;
  const files = parse(text);
  // which folder, and why it's every change when the last turn was asked for
  const where = scope === "turn" && d.whole ? `${d.dir}\nNo turn seen yet: every change since the last commit.` : d.dir;
  if (!files.length) return message(body, `No changes.\n${where}`);
  const top = body.querySelector(".diff")?.scrollTop ?? 0;
  const root = el("div", "diff");
  root.append(el("div", "diff-where", where));
  for (const f of files) {
    const head = el("div", "df");
    head.append(el("span", "p", f.path), el("span", "n", f.binary ? "binary" : `+${f.add} −${f.del}`));
    root.append(head);
    for (const l of f.lines) root.append(el("div", `dl ${l.kind}`, l.text));
  }
  if (truncated(text)) root.append(el("div", "diff-cut", "Cut at 1 MB: the rest is in git."));
  body.replaceChildren(root);
  root.scrollTop = top;
}

/** Header: which session, and the last-turn / all toggle. Re-reads the diff when the session's status moves. */
export function update(pane: HTMLElement, paneId: string, v: View | undefined) {
  const head = pane.querySelector<HTMLElement>(".ph")!;
  const body = pane.querySelector<HTMLElement>(".pb")!;
  head.className = "ph diffh";
  const tabs = el("span", "scope");
  for (const [s, label] of [["turn", "last turn"], ["all", "all"]] as [Scope, string][]) {
    const b = el("button", (scopes.get(paneId) ?? "turn") === s ? "on" : "", label);
    b.onmousedown = (e) => e.stopPropagation();
    b.onclick = () => {
      scopes.set(paneId, s);
      update(pane, paneId, v);
      void load(body, paneId);
    };
    tabs.append(b);
  }
  head.replaceChildren(el("span", "name", `diff · ${v?.name ?? ""}`), tabs);
  const status = v?.status ?? "";
  if (seen.get(paneId) !== status) {
    seen.set(paneId, status);
    void load(body, paneId);
  }
}
