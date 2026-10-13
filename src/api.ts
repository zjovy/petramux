import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Status = "starting" | "shell" | "asleep" | "working" | "needs_you" | "idle" | "exited";

export interface View {
  id: string;
  name: string;
  repo: string;
  workdir: string;
  parent: string | null;
  status: Status;
  since: number;
  createdAt: number;
  note: string | null;
}

export interface Past {
  claudeId: string;
  title: string;
  workdir: string;
  modifiedMs: number;
}

export interface Boot {
  sessions: View[];
  recentRepos: string[];
  claudeFound: boolean;
  layout: unknown;
  focus: string | null;
  shell: string;
}

export const api = {
  boot: () => invoke<Boot>("boot"),
  attach: (id: string, onText: (text: string) => void) => {
    const output = new Channel<string>();
    output.onmessage = onText;
    return invoke<void>("attach", { id, output });
  },
  select: (id: string) => invoke<View>("select_session", { id }),
  create: (prompt: string, repo: string, worktree: boolean) =>
    invoke<View>("new_session", { prompt, repo, worktree }),
  write: (id: string, data: string) => invoke<void>("write", { id, data }),
  resize: (id: string, cols: number, rows: number) => invoke<void>("resize", { id, cols, rows }),
  rename: (id: string, name: string) => invoke<View>("rename", { id, name }),
  close: (id: string) => invoke<void>("close", { id }),
  saveLayout: (layout: unknown, focus: string | null) => invoke<void>("save_layout", { layout, focus }),
  listPast: (repo: string) => invoke<Past[]>("list_past", { repo }),
  resumePast: (repo: string, p: Past) =>
    invoke<View>("resume_past", { repo, claudeId: p.claudeId, title: p.title, workdir: p.workdir }),
  pickFolder: () => invoke<string | null>("pick_folder"),
  ask: (message: string, ok: string) => invoke<boolean>("ask", { message, ok }),
  diff: (id: string, scope: "turn" | "all") => invoke<string>("diff", { id, scope }),
  onSessions: (f: (views: View[]) => void) => listen<View[]>("sessions_changed", (e) => f(e.payload)),
  onSession: (f: (view: View) => void) => listen<View>("session_changed", (e) => f(e.payload)),
};
