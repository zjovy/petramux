/** The chat panel: the workspace on screen's chat, beside its panes. You post as "user". */
import { api, type Chat, type Msg } from "./api";

const panel = document.getElementById("chat")!;
const title = panel.querySelector<HTMLElement>(".chat-name")!;
const log = panel.querySelector<HTMLElement>(".chat-log")!;
const input = panel.querySelector<HTMLTextAreaElement>("textarea")!;

let shown: Chat | null = null;
let loading = 0;

export const tag = (name: string) => "#" + name.trim().split(/\s+/).join("-").toLowerCase();

const clock = (ts: number) => new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

function line(m: Msg) {
  const row = document.createElement("div");
  row.className = `chat-msg${m.from === "user" ? " me" : ""}${m.from === "petramux" ? " app" : ""}`;
  const head = document.createElement("div");
  head.className = "chat-head";
  const who = document.createElement("span");
  who.className = "chat-from";
  who.textContent = m.from === "user" ? "You" : m.from;
  const when = document.createElement("span");
  when.className = "chat-time";
  when.textContent = clock(m.ts);
  head.append(who, when);
  const text = document.createElement("div");
  text.className = "chat-text";
  // @mentions stand out; everything else is plain text
  for (const part of m.text.split(/(@[\w-]+)/)) {
    if (part.startsWith("@")) text.append(Object.assign(document.createElement("b"), { textContent: part }));
    else text.append(part);
  }
  row.append(head, text);
  return row;
}

const atBottom = () => log.scrollHeight - log.scrollTop - log.clientHeight < 40;

/** Shows `chat` (or hides the panel): its log is read again when the chat on screen changes. */
export function show(chat: Chat | null, open: boolean) {
  panel.hidden = !open || !chat;
  const changed = chat?.id !== shown?.id;
  shown = chat;
  if (!chat) return;
  title.textContent = tag(chat.name);
  title.title = `${chat.name}: double-click to rename`;
  input.placeholder = `Message ${tag(chat.name)} (@name or @all to deliver)`;
  if (!changed) return;
  log.replaceChildren();
  const mine = ++loading;
  void api.chatLog(chat.id).then((msgs) => {
    if (mine !== loading) return; // another chat came on screen meanwhile
    log.replaceChildren(...msgs.map(line));
    log.scrollTop = log.scrollHeight;
  });
}

/** A message was posted somewhere: shown if it's this chat's. */
export function message(m: Msg) {
  if (m.chat !== shown?.id) return;
  const stick = atBottom();
  log.append(line(m));
  if (stick) log.scrollTop = log.scrollHeight;
}

export function focus() {
  input.focus();
}

async function send() {
  const text = input.value.trim();
  if (!text || !shown) return;
  try {
    await api.chatSay(shown.id, text);
    input.value = "";
  } catch (e) {
    log.append(line({ ts: Date.now(), from: "petramux", text: `Not sent: ${e}`, chat: shown.id }));
    log.scrollTop = log.scrollHeight;
  }
}

/** Wires the panel's input, close button and rename; `closed` runs when it's closed from inside. */
export function init(closed: () => void) {
  input.addEventListener("keydown", (e) => {
    if (!e.metaKey) e.stopPropagation(); // ⌘ shortcuts still reach the window's listener
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      void send();
    } else if (e.key === "Escape") closed();
  });
  panel.querySelector<HTMLElement>(".chat-close")!.onclick = closed;
  title.ondblclick = () => {
    if (!shown) return;
    const chat = shown;
    const box = Object.assign(document.createElement("input"), { className: "chat-rename", value: chat.name });
    title.replaceWith(box);
    box.select();
    let done = false;
    const finish = (save: boolean) => {
      if (done) return;
      done = true;
      box.replaceWith(title);
      if (save && box.value.trim() && box.value.trim() !== chat.name) void api.renameChat(chat.id, box.value).catch(() => {});
    };
    box.onkeydown = (e) => {
      e.stopPropagation();
      if (e.key === "Enter") finish(true);
      if (e.key === "Escape") finish(false);
    };
    box.onblur = () => finish(true);
  };
}
