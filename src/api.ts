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
  /** Claude here is older than the one installed now. */
  outdated: boolean;
}

/** A workspace's chat: it follows the sessions in the workspace. */
export interface Chat {
  id: string;
  name: string;
  members: string[];
}

/** One chat message: from a session (its name), "user" (you) or "petramux" (a note). */
export interface Msg {
  ts: number;
  from: string;
  text: string;
  chat: string;
}

export interface Past {
  claudeId: string;
  title: string;
  workdir: string;
  modifiedMs: number;
}

export interface Appearance {
  theme: string | null;
  fontFamily: string[];
  fontSize: number | null;
  colors: Record<string, string>;
}

/** A diff pane's changes, the folder they're in, and whether they're every change since the last
 *  commit (asked for, or no turn seen yet) rather than the last turn's. */
export interface Diff {
  text: string;
  dir: string;
  whole: boolean;
}

export interface Settings {
  newSessionFolder: string | null;
  quitStopsSessions: boolean;
  notifyNeedsYou: boolean;
  notifyDone: boolean;
  claudeMatchesTheme: boolean;
  /** Workspace chat: messages between sessions in a row before delivery pauses; 0: never. */
  chatPauseAfter: number;
}

export interface Boot {
  sessions: View[];
  recentRepos: string[];
  claudeFound: boolean;
  layout: unknown;
  focus: string | null;
  shell: string;
  settings: Settings;
  /** Terminals kept running from before this window opened. */
  restored: string[];
}

export const api = {
  boot: () => invoke<Boot>("boot"),
  attach: (id: string, onText: (text: string) => void) => {
    const output = new Channel<string>();
    output.onmessage = onText;
    return invoke<void>("attach", { id, output });
  },
  select: (id: string) => invoke<View>("select_session", { id }),
  create: (prompt: string, repo: string, worktree: boolean, parent: string | null = null) =>
    invoke<View>("new_session", { prompt, repo, worktree, parent }),
  write: (id: string, data: string) => invoke<void>("write", { id, data }),
  resize: (id: string, cols: number, rows: number) => invoke<void>("resize", { id, cols, rows }),
  rename: (id: string, name: string) => invoke<View>("rename", { id, name }),
  close: (id: string) => invoke<void>("close", { id }),
  saveLayout: (layout: unknown, focus: string | null) => invoke<void>("save_layout", { layout, focus }),
  listPast: (repo: string) => invoke<Past[]>("list_past", { repo }),
  resumePast: (repo: string, p: Past) =>
    invoke<View>("resume_past", { repo, claudeId: p.claudeId, title: p.title, workdir: p.workdir }),
  appearance: () => invoke<Appearance>("appearance"),
  fork: (id: string) => invoke<View>("fork", { id }),
  restartClaude: (id: string) => invoke<void>("restart_claude", { id }),
  reopenClosed: () => invoke<View>("reopen_closed"),
  openLink: (target: string, base: string, line?: number, col?: number) =>
    invoke<void>("open_link", { target, base, line: line ?? null, col: col ?? null }),
  existingPaths: (base: string, paths: string[]) => invoke<boolean[]>("existing_paths", { base, paths }),
  updateReady: () => invoke<boolean>("update_ready"),
  openWindow: (id: string, name: string, x?: number, y?: number) => invoke<void>("open_window", { id, name, x: x ?? null, y: y ?? null }),
  focusWindow: (id: string) => invoke<void>("focus_window", { id }),
  closeWindow: (id: string) => invoke<void>("close_window", { id }),
  /** The main window: a session's own window closed (the session comes back). */
  onWindowClosed: (f: (id: string) => void) => listen<string>("window_closed", (e) => f(e.payload)),
  place: (id: string) => invoke<{ dir: string; branch: string | null }>("place", { id }),
  restart: () => invoke<void>("restart"),
  setClaudeTheme: (mode: "dark-ansi" | "light-ansi" | null) => invoke<void>("set_claude_theme", { mode }),
  saveDrop: (name: string, bytes: Uint8Array) => invoke<string>("save_drop", bytes, { headers: { name } }),
  themes: () => invoke<string[]>("themes"),
  theme: (name: string) => invoke<Record<string, string> | null>("theme", { name }),
  pickFolder: () => invoke<string | null>("pick_folder"),
  ask: (message: string, ok: string) => invoke<boolean>("ask", { message, ok }),
  diff: (id: string, scope: "turn" | "all") => invoke<Diff>("diff", { id, scope }),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  onMenu: (f: (item: string) => void) => listen<string>("menu", (e) => f(e.payload)),
  chats: () => invoke<Chat[]>("chats"),
  chatLog: (chat: string) => invoke<Msg[]>("chat_log", { chat }),
  chatSay: (chat: string, text: string) => invoke<void>("chat_say", { chat, text }),
  renameChat: (chat: string, name: string) => invoke<void>("rename_chat", { chat, name }),
  onChatMessage: (f: (msg: Msg) => void) => listen<Msg>("chat_message", (e) => f(e.payload)),
  onChats: (f: (chats: Chat[]) => void) => listen<Chat[]>("chats_changed", (e) => f(e.payload)),
  onSessions: (f: (views: View[]) => void) => listen<View[]>("sessions_changed", (e) => f(e.payload)),
  onSession: (f: (view: View) => void) => listen<View>("session_changed", (e) => f(e.payload)),
};
