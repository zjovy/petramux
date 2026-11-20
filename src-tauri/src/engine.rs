use crate::chat::{self, Chat, Msg};
use crate::integration;
use crate::past::{self, Past};
use crate::paths::Paths;
use crate::pty::{Probe, PtyManager, Spawn};
use crate::shellenv::Env;
use crate::sock::{Handler, LsRow, Request, Response};
use crate::state::{self, Session, Status};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub workdir: String,
    pub parent: Option<String>,
    pub status: Status,
    pub since: u64,
    pub created_at: u64,
    pub note: Option<String>,
    /// Claude here is an older version than the one installed now.
    pub outdated: bool,
}

/// What a diff pane shows: the changes, the folder they're in, and whether they're every change
/// since the last commit (asked for, or no turn seen yet) rather than the last turn's.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diff {
    pub text: String,
    pub dir: String,
    pub whole: bool,
}

/// Where a session works: its folder and git branch.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Place {
    pub dir: String,
    pub branch: Option<String>,
}

pub enum Attention {
    NeedsYou,
    Done,
}

pub trait Sink: Send + Sync + 'static {
    fn sessions_changed(&self, views: Vec<View>);
    fn session_changed(&self, view: View);
    fn output(&self, id: &str, text: String);
    fn attention(&self, view: &View, kind: Attention);
    /// A message was posted to a workspace chat.
    fn chat_message(&self, _msg: &Msg) {}
    /// The workspaces' chats changed (a workspace came or went, or a chat was renamed).
    fn chats_changed(&self, _chats: Vec<Chat>) {}
}

pub struct NewSession {
    pub prompt: String,
    pub repo: String,
    pub worktree: bool,
    pub name: Option<String>,
    pub parent: Option<String>,
}

/// Everything the engine needs from the outside world (the app or a test).
pub struct Setup {
    pub paths: Paths,
    pub env: Env,
    pub claude: Option<PathBuf>,
    pub roost_bin: PathBuf,
    pub shell: PathBuf,
}

struct Live {
    status: Status,
    since: u64,
    claude_pid: Option<u32>,
    pending: Option<String>,
    typed_at: Option<Instant>,
    /// When you last typed in the terminal (ms).
    input_at: u64,
    outdated: bool,
    /// Chat messages that name this session, not yet handed over: (chat id, text).
    inbox: Vec<(String, String)>,
    /// Its chat is read up to here (ms: the last message it was shown).
    read_at: u64,
    /// A `petramux chat --wait` waiting for this session to be named: its pid, and how to wake it.
    listener: Option<(u32, mpsc::Sender<()>)>,
}

struct Inner {
    state: state::State,
    live: HashMap<String, Live>,
    guard: chat::Guard,
}

pub struct Core {
    inner: Mutex<Inner>,
    setup: Setup,
    ptys: PtyManager,
    sink: Arc<dyn Sink>,
    size: Mutex<(u16, u16)>,
    quitting: AtomicBool,
    /// Terminals kept running from before this app started (their screens come back as replays).
    restored: Vec<String>,
    /// Sessions closed since the app started, last last: ⌘⇧T brings them back.
    closed: Mutex<Vec<Session>>,
    me: Weak<Core>,
}

fn live(status: Status) -> Live {
    Live {
        status,
        since: state::now_ms(),
        claude_pid: None,
        pending: None,
        typed_at: None,
        input_at: 0,
        outdated: false,
        inbox: vec![],
        read_at: state::now_ms(),
        listener: None,
    }
}

fn alive(pid: u32) -> bool {
    let signalled = unsafe { libc::kill(pid as i32, 0) } == 0;
    signalled || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The program a process runs (for Claude, its version's file: Claude updates by adding a newer
/// one beside it and pointing `claude` there).
fn exe_of(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| {
        buf.truncate(n as usize);
        PathBuf::from(std::ffi::OsString::from_vec(buf))
    })
}

/// Running an older version than the one installed: another file in the same versions folder.
fn older(running: &Path, installed: &Path) -> bool {
    running != installed && running.parent() == installed.parent()
}

/// An out-of-date Claude restarts by itself once its turn is over, it has sat idle a minute, and
/// nothing was typed since (that could be a prompt you haven't sent).
fn restart_due(l: &Live, outdated: bool, now: u64) -> bool {
    outdated
        && l.status == Status::Idle
        && l.claude_pid.is_some()
        && now.saturating_sub(l.since) >= 60_000
        && l.input_at < l.since
}

impl Core {
    pub fn new(setup: Setup, sink: Arc<dyn Sink>) -> Arc<Core> {
        if let Err(e) = integration::install(&setup.paths, &setup.roost_bin) {
            eprintln!("roost: could not install shell integration: {e}");
        }
        let mut state = state::load(&setup.paths.state());
        // The helper keeps terminals running while the app is closed: those rows are live, not asleep.
        let ptys = PtyManager::new(setup.paths.pty_sock(), Some(setup.roost_bin.clone()));
        let running = ptys.running();
        let mut live_rows = HashMap::new();
        for s in &mut state.sessions {
            let term = running.iter().find(|t| t.id == s.id);
            // Claude still has the keyboard: something other than the shell leads the foreground.
            let claude = term.and_then(|t| t.fg.filter(|fg| Some(*fg as u32) != t.pid)).filter(|_| s.claude_running);
            let mut l = live(match (term, claude) {
                // a question still waits for you (no hook fires until it's answered); "working" comes
                // back from the next hook, never from a stale save
                (Some(_), Some(_)) => s.claude_status.filter(|st| *st == Status::NeedsYou).unwrap_or(Status::Idle),
                (Some(_), None) => Status::Shell,
                (None, _) if s.exited => Status::Exited,
                (None, _) => Status::Asleep,
            });
            // what was typed here before this launch is unknown: no restarting it by itself until its next turn
            l.input_at = l.since;
            if term.is_some() {
                l.claude_pid = claude.map(|p| p as u32);
                // Claude ended while the app was closed
                s.claude_running &= claude.is_some();
                s.exited = false;
            }
            live_rows.insert(s.id.clone(), l);
        }
        // workspaces saved before chats (or since the last save) get theirs now
        let groups = state.layout.as_ref().map(chat::groups).unwrap_or_default();
        let name_of = |id: &str| state.sessions.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
        state.chats = chat::assign(&state.chats, &groups, name_of);
        let core = Arc::new_cyclic(|me| Core {
            inner: Mutex::new(Inner { state, live: live_rows, guard: chat::Guard::default() }),
            setup,
            ptys,
            sink,
            size: Mutex::new((120, 36)),
            quitting: AtomicBool::new(false),
            closed: Mutex::new(Vec::new()),
            restored: running.iter().map(|t| t.id.clone()).collect(),
            me: me.clone(),
        });
        for t in running {
            if core.session(&t.id).is_some() {
                let (out, exit) = core.listeners(&t.id);
                if core.ptys.attach(&t.id, out, exit).is_err() {
                    core.on_stream_end(&t.id); // it ended (or the helper did) between the list and the attach
                }
            } else {
                core.ptys.kill(&t.id); // its row is gone
            }
        }
        core.save(&core.inner.lock().unwrap());
        // Claude killed without SessionEnd (Ctrl-C, crash) still returns its row to the shell.
        let weak = Arc::downgrade(&core);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(2));
            let Some(core) = weak.upgrade() else { break };
            core.reap_claude();
            core.check_versions();
        });
        core
    }

    pub fn handler(self: &Arc<Self>) -> Handler {
        let me = self.clone();
        Arc::new(move |req| me.handle(req))
    }

    /// A program on the user's PATH (their login shell's).
    pub fn find_bin(&self, name: &str) -> Option<PathBuf> {
        crate::shellenv::find_on_path(&self.setup.env, name)
    }

    /// The terminals kept running from before this app started: the window redraws them once
    /// their screens are replayed (see `terminals.ts`).
    pub fn restored(&self) -> Vec<String> {
        self.restored.clone()
    }

    pub fn claude_found(&self) -> bool {
        self.setup.claude.is_some()
    }

    pub fn shell_name(&self) -> String {
        self.setup.shell.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "shell".into())
    }

    pub fn recent_repos(&self) -> Vec<String> {
        self.inner.lock().unwrap().state.recent_repos.clone()
    }

    pub fn views(&self) -> Vec<View> {
        let inner = self.inner.lock().unwrap();
        inner.state.sessions.iter().filter_map(|s| view(&inner, &s.id)).collect()
    }

    pub fn layout(&self) -> (Option<Value>, Option<String>) {
        let inner = self.inner.lock().unwrap();
        (inner.state.layout.clone(), inner.state.focus.clone())
    }

    pub fn settings(&self) -> state::Settings {
        self.inner.lock().unwrap().state.settings.clone()
    }

    pub fn set_settings(&self, settings: state::Settings) {
        let mut inner = self.inner.lock().unwrap();
        inner.state.settings = settings;
        self.save(&inner);
    }

    pub fn save_layout(&self, layout: Option<Value>, focus: Option<String>) {
        let changed = {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            let groups = layout.as_ref().map(chat::groups).unwrap_or_default();
            let sessions = &inner.state.sessions;
            let name_of = |id: &str| sessions.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
            let chats = chat::assign(&inner.state.chats, &groups, name_of);
            let changed = chats != inner.state.chats;
            inner.state.chats = chats;
            inner.state.layout = layout;
            inner.state.focus = focus;
            self.save(inner);
            changed.then(|| inner.state.chats.clone())
        };
        if let Some(chats) = changed {
            self.sink.chats_changed(chats);
        }
    }

    pub fn chats(&self) -> Vec<Chat> {
        self.inner.lock().unwrap().state.chats.clone()
    }

    pub fn rename_chat(&self, chat: &str, name: &str) -> Result<(), String> {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            return Err("a chat needs a name".into());
        }
        let chats = {
            let mut inner = self.inner.lock().unwrap();
            let c = inner.state.chats.iter_mut().find(|c| c.id == chat).ok_or("no such chat")?;
            c.name = name.chars().take(60).collect();
            self.save(&inner);
            inner.state.chats.clone()
        };
        self.sink.chats_changed(chats);
        Ok(())
    }

    /// The chat of the workspace a session is in.
    fn chat_of(&self, session: &str) -> Option<Chat> {
        self.inner.lock().unwrap().state.chats.iter().find(|c| c.members.iter().any(|m| m == session)).cloned()
    }

    /// The last `n` messages of a chat.
    pub fn chat_log(&self, chat: &str, n: usize) -> Vec<Msg> {
        chat::tail(&self.setup.paths.dir, chat, n)
    }

    /// A session posts to its workspace's chat (`petramux say`).
    pub fn say(&self, session: &str, text: &str) -> Result<(), String> {
        let c = self.chat_of(session).ok_or("this session isn't in a workspace")?;
        self.post(&c.id, Some(session), text)
    }

    /// You post to a chat (its pane in the window).
    pub fn say_user(&self, chat: &str, text: &str) -> Result<(), String> {
        self.post(chat, None, text)
    }

    /// Logs a message, shows it, and hands it to the sessions it names: now if their turn is over,
    /// else when it is. `from`: the session posting, or none for you.
    fn post(&self, chat_id: &str, from: Option<&str>, text: &str) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("nothing to say".into());
        }
        let now = state::now_ms();
        let (msg, note, to) = {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            let c = inner.state.chats.iter().find(|c| c.id == chat_id).cloned().ok_or("no such chat")?;
            let name_of = |id: &str| inner.state.sessions.iter().find(|s| s.id == id).map(|s| s.name.clone());
            let sender = match from {
                Some(id) => name_of(id).ok_or("no such session")?,
                None => chat::USER.into(),
            };
            let verdict = match from {
                Some(id) => inner.guard.post(chat_id, id, now)?,
                None => {
                    inner.guard.user_posted(chat_id);
                    chat::Verdict::Deliver
                }
            };
            // Claude sessions only: a paste into a plain terminal would run as a command.
            let members: Vec<(String, String)> = c
                .members
                .iter()
                .filter(|id| inner.live.get(*id).is_some_and(|l| !matches!(l.status, Status::Shell | Status::Exited)))
                .filter_map(|id| name_of(id).map(|n| (id.clone(), n)))
                .collect();
            let msg = Msg { ts: now, from: sender.clone(), text: text.into(), chat: chat_id.into() };
            chat::append(&self.setup.paths.dir, chat_id, &msg).map_err(|e| format!("could not save the message: {e}"))?;
            let (to, note) = match verdict {
                chat::Verdict::Deliver => (chat::recipients(text, &members, from), None),
                chat::Verdict::Hold { first } => {
                    let note = first.then(|| Msg {
                        ts: now,
                        from: chat::APP.into(),
                        text: format!(
                            "{} messages between sessions in a row: delivery is paused until you post here.",
                            chat::STREAK
                        ),
                        chat: chat_id.into(),
                    });
                    (vec![], note)
                }
            };
            let line = chat::delivered(&c.name, &sender, text);
            for id in &to {
                if let Some(l) = inner.live.get_mut(id) {
                    l.inbox.push((chat_id.into(), line.clone()));
                }
            }
            (msg, note, to)
        };
        self.sink.chat_message(&msg);
        if let Some(note) = note {
            let _ = chat::append(&self.setup.paths.dir, chat_id, &note);
            self.sink.chat_message(&note);
        }
        for id in to {
            self.deliver(id);
        }
        Ok(())
    }

    /// Hands a session the messages that name it: its listener reads them, or else they're
    /// pasted once its turn is over (its hooks hand them over during one).
    fn deliver(&self, id: String) {
        {
            let mut inner = self.inner.lock().unwrap();
            let Some(l) = inner.live.get_mut(&id) else { return };
            if l.inbox.is_empty() {
                return;
            }
            if let Some((pid, wake)) = l.listener.take() {
                if alive(pid) && wake.send(()).is_ok() {
                    l.inbox.clear(); // the listener reads them from the log
                    return;
                }
            }
        }
        self.flush_inbox_later(id);
    }

    /// `petramux chat --wait`: returns once a message names this session (at once if one waits).
    /// `pid` is the waiting command's: one that was killed stops counting as a listener.
    fn listen(&self, id: &str, pid: u32) -> Result<(), String> {
        let (wake, woken) = mpsc::channel();
        {
            let mut inner = self.inner.lock().unwrap();
            let l = inner.live.get_mut(id).ok_or("no such session")?;
            if l.listener.as_ref().is_some_and(|(p, _)| *p != pid && alive(*p)) {
                return Err("already listening: another `petramux chat --wait` is running".into());
            }
            if !l.inbox.is_empty() {
                l.inbox.clear();
                l.listener = None;
                return Ok(());
            }
            l.listener = Some((pid, wake));
        }
        loop {
            match woken.recv_timeout(Duration::from_secs(2)) {
                Ok(()) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("the session ended".into()),
                Err(mpsc::RecvTimeoutError::Timeout) if !alive(pid) => {
                    let mut inner = self.inner.lock().unwrap();
                    if let Some(l) = inner.live.get_mut(id).filter(|l| l.listener.as_ref().is_some_and(|(p, _)| *p == pid)) {
                        l.listener = None;
                    }
                    return Err("stopped".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// What a session reads in its chat: the last `tail` messages, or more to include all it
    /// hasn't read (up to 100), and how many of them are new. They count as read.
    fn read_chat(&self, id: &str, tail: usize) -> Result<(Chat, Vec<Msg>, usize), String> {
        let mut inner = self.inner.lock().unwrap();
        let inner = &mut *inner;
        let c = inner.state.chats.iter().find(|c| c.members.iter().any(|m| m == id)).cloned().ok_or("this session isn't in a workspace")?;
        let me = inner.state.sessions.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
        let l = inner.live.get_mut(id).ok_or("no such session")?;
        // under the lock: a message is logged and queued for its readers under it too
        let mut all = chat::tail(&self.setup.paths.dir, &c.id, 100.max(tail));
        let new = all.iter().position(|m| m.ts > l.read_at && m.from != me).map_or(0, |i| all.len() - i);
        let shown = tail.max(new).min(all.len());
        all.drain(..all.len() - shown);
        if let Some(last) = all.last() {
            l.read_at = l.read_at.max(last.ts);
        }
        l.inbox.clear();
        Ok((c, all, new.min(shown)))
    }

    /// About to stop, in a workspace with other Claudes, and no listener running.
    fn should_listen(&self, id: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        let listening = inner.live.get(id).and_then(|l| l.listener.as_ref()).is_some_and(|(p, _)| alive(*p));
        let others = inner.state.chats.iter().find(|c| c.members.iter().any(|m| m == id)).is_some_and(|c| {
            c.members.iter().any(|m| m != id && inner.live.get(m).is_some_and(|l| !matches!(l.status, Status::Shell | Status::Exited)))
        });
        others && !listening
    }

    fn flush_inbox_later(&self, id: String) {
        if let Some(core) = self.me.upgrade() {
            thread::spawn(move || core.flush_inbox(&id));
        }
    }

    /// Pastes a session's waiting chat messages as one prompt, unless it's mid-turn or asking you
    /// something (then they wait for the turn to end: see `set_status`).
    fn flush_inbox(&self, id: &str) {
        let waiting = {
            let mut inner = self.inner.lock().unwrap();
            let Some(l) = inner.live.get_mut(id) else { return };
            if matches!(l.status, Status::Working | Status::NeedsYou) || l.inbox.is_empty() {
                return;
            }
            std::mem::take(&mut l.inbox)
        };
        let text = waiting.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n\n");
        if let Err(e) = self.paste(id, id, &text) {
            let name = self.session(id).map(|s| s.name).unwrap_or_default();
            let mut chats: Vec<&String> = waiting.iter().map(|(c, _)| c).collect();
            chats.dedup();
            for c in chats {
                let note = Msg { ts: state::now_ms(), from: chat::APP.into(), text: format!("Couldn't deliver to {name}: {e}"), chat: c.clone() };
                let _ = chat::append(&self.setup.paths.dir, c, &note);
                self.sink.chat_message(&note);
            }
        }
    }

    fn save(&self, inner: &Inner) {
        if let Err(e) = state::save(&self.setup.paths.state(), &inner.state) {
            eprintln!("roost: could not save state: {e}");
        }
    }

    fn emit_all(&self) {
        self.sink.sessions_changed(self.views());
    }

    /// What changed in the session's folder: since the last prompt ("turn"), or since HEAD ("all").
    /// The folder a session works in, and its turn's start: where Claude is now, by its
    /// conversation (the repo's top, in one), else where it started.
    fn work_dir(&self, id: &str) -> Result<(String, Option<String>), String> {
        let (workdir, base, claude_id) = {
            let inner = self.inner.lock().unwrap();
            let s = inner.state.sessions.iter().find(|s| s.id == id).ok_or("no such session")?;
            (s.workdir.clone(), s.turn_base.clone(), s.claude_id.clone())
        };
        let now = Some(claude_id)
            .filter(|c| !c.is_empty())
            .and_then(|c| past::current_dir(&self.projects(), &c))
            .filter(|d| Path::new(d).is_dir())
            .map(|d| crate::git::toplevel(Path::new(&d)).unwrap_or(d));
        Ok((now.unwrap_or(workdir), base))
    }

    pub fn place(&self, id: &str) -> Result<Place, String> {
        let (dir, _) = self.work_dir(id)?;
        let branch = crate::git::branch(Path::new(&dir));
        Ok(Place { dir, branch })
    }

    pub fn diff(&self, id: &str, scope: &str) -> Result<Diff, String> {
        let (dir, base) = self.work_dir(id)?;
        // no turn seen yet: every change since the last commit
        let base = base.filter(|_| scope != "all");
        let text = crate::git::diff(Path::new(&dir), base.as_deref()).map_err(|e| format!("{e}\n{dir}"))?;
        Ok(Diff { text, dir, whole: base.is_none() })
    }

    /// True when the status changed (and the change was sent out).
    fn set_status(&self, id: &str, status: Status) -> bool {
        let (v, attention) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(l) = inner.live.get_mut(id) else { return false };
            if l.status == status || l.status == Status::Exited {
                return false;
            }
            let prev = l.status;
            l.status = status;
            l.since = state::now_ms();
            if matches!(status, Status::Working | Status::NeedsYou | Status::Idle) {
                if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                    if s.claude_status != Some(status) {
                        s.claude_status = Some(status);
                        self.save(&inner);
                    }
                }
            }
            let attention = match (prev, status) {
                (_, Status::NeedsYou) => Some(Attention::NeedsYou),
                (Status::Working, Status::Idle) => Some(Attention::Done),
                _ => None,
            };
            (view(&inner, id), attention)
        };
        let Some(v) = v else { return false };
        if status == Status::Idle {
            self.deliver(id.into()); // chat messages that waited for the turn to end
        }
        if let Some(a) = attention {
            self.sink.attention(&v, a);
        }
        self.sink.session_changed(v);
        true
    }

    /// An id, an exact name, or a name compared loosely (see `state::name_key`).
    fn resolve(&self, target: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        let sessions = &inner.state.sessions;
        let key = state::name_key(target);
        sessions
            .iter()
            .find(|s| s.id == target || s.name == target)
            .or_else(|| sessions.iter().find(|s| !key.is_empty() && state::name_key(&s.name) == key))
            .map(|s| s.id.clone())
    }

    fn session(&self, id: &str) -> Option<Session> {
        self.inner.lock().unwrap().state.sessions.iter().find(|s| s.id == id).cloned()
    }

    fn status(&self, id: &str) -> Option<Status> {
        self.inner.lock().unwrap().live.get(id).map(|l| l.status)
    }

    fn view(&self, id: &str) -> Option<View> {
        view(&self.inner.lock().unwrap(), id)
    }

    pub fn new_session(&self, req: NewSession) -> Result<View, String> {
        self.create(req, None)
    }

    /// A copy of the row's conversation in a row of its own, as its child. (Claude's own /fork
    /// puts the copy in Claude's background service, where this app can't see or show it.)
    pub fn fork(&self, id: &str) -> Result<View, String> {
        let s = self.session(id).ok_or("no such session")?;
        let (conversation, dir) = (s.claude_id.clone(), s.workdir.clone());
        if conversation.is_empty() || !past::saved(&self.projects(), &conversation) {
            return Err("Nothing to fork yet: send Claude a prompt first.".into());
        }
        let req = NewSession {
            prompt: String::new(),
            repo: dir,
            worktree: false,
            name: Some(format!("{} fork", s.name)),
            parent: Some(s.id.clone()),
        };
        self.create(req, Some(integration::fork_cmd(&conversation)))
    }

    /// A new row; `first` is typed into its shell instead of the prompt's claude command.
    fn create(&self, mut req: NewSession, first: Option<String>) -> Result<View, String> {
        if req.repo.is_empty() {
            req.repo = self.setup.env.get("HOME").cloned().unwrap_or_default(); // a plain terminal opens at home
        }
        if !Path::new(&req.repo).is_dir() {
            return Err(format!("no such folder: {}", req.repo));
        }
        let prompt = req.prompt.trim();
        if !prompt.is_empty() && self.setup.claude.is_none() {
            return Err("Claude Code not found. Install it, or set ROOST_CLAUDE_BIN.".into());
        }
        let folder = Path::new(&req.repo).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let id = uuid::Uuid::new_v4().to_string();
        let prompt_file = integration::prompt_file(&self.setup.paths, &id);
        if !prompt.is_empty() {
            std::fs::create_dir_all(prompt_file.parent().unwrap())
                .and_then(|_| std::fs::write(&prompt_file, prompt))
                .map_err(|e| format!("could not save the prompt: {e}"))?;
        }
        let session = {
            let mut inner = self.inner.lock().unwrap();
            let taken: Vec<&str> = inner.state.sessions.iter().map(|s| s.name.as_str()).collect();
            let label = req.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
                if prompt.is_empty() { folder.clone() } else { prompt.to_string() }
            });
            let s = Session {
                id,
                claude_id: String::new(),
                name: state::unique_name(&state::slug(&label), &taken),
                repo: req.repo.clone(),
                workdir: req.repo.clone(),
                parent: req.parent,
                created_at: state::now_ms(),
                exited: false,
                claude_running: false,
                note: None,
                turn_base: None,
                claude_status: None,
            };
            inner.state.sessions.push(s.clone());
            inner.live.insert(s.id.clone(), live(Status::Shell));
            let recent = &mut inner.state.recent_repos;
            recent.retain(|r| r != &req.repo);
            recent.insert(0, req.repo.clone());
            recent.truncate(10);
            self.save(&inner);
            s
        };
        self.emit_all();
        let line = first.or_else(|| {
            (!prompt.is_empty())
                .then(|| integration::claude_cmd(&prompt_file, req.worktree.then_some(session.name.as_str())))
        });
        self.spawn(&session, line)?;
        self.view(&session.id).ok_or_else(|| "session vanished".into())
    }

    /// What a terminal's output and its end do: shown in its pane, and watched for status.
    fn listeners(&self, id: &str) -> (impl Fn(String) + Send + 'static, impl FnOnce() + Send + 'static) {
        let sink = self.sink.clone();
        let (out_id, exit_id) = (id.to_string(), id.to_string());
        let (out_me, exit_me) = (self.me.clone(), self.me.clone());
        let out = move |text: String| {
            if let Some(core) = out_me.upgrade() {
                core.on_output(&out_id, &text);
            }
            sink.output(&out_id, text)
        };
        let exit = move || {
            if let Some(core) = exit_me.upgrade() {
                core.on_stream_end(&exit_id);
            }
        };
        (out, exit)
    }

    /// Starts the row's shell; `line` is typed once the shell is ready.
    fn spawn(&self, s: &Session, line: Option<String>) -> Result<(), String> {
        let (cols, rows) = *self.size.lock().unwrap();
        // Empty when Claude wasn't found: the shim then runs whatever `claude` is on PATH, untracked.
        let claude = self.setup.claude.clone().unwrap_or_default();
        let spec = Spawn {
            program: self.setup.shell.clone(),
            args: vec!["-l".into()],
            cwd: PathBuf::from(&s.repo),
            env: integration::session_env(&self.setup.env, &self.setup.paths, &self.setup.roost_bin, &claude, &self.setup.shell, &s.id),
            cols,
            rows,
        };
        if let Some(l) = self.inner.lock().unwrap().live.get_mut(&s.id) {
            l.pending = line;
            l.claude_pid = None;
        }
        let (out, exit) = self.listeners(&s.id);
        let result = self.ptys.spawn(&s.id, spec, out, exit);
        if let Err(e) = result {
            if e.kind() == io::ErrorKind::AlreadyExists {
                return Ok(()); // someone else already started it
            }
            self.on_exit(&s.id);
            return Err(format!("could not start {}: {e}", self.shell_name()));
        }
        if self.session(&s.id).is_none() {
            self.ptys.kill(&s.id); // the row was closed while we were spawning
            return Ok(());
        }
        // zsh reports its first prompt (ShellReady); other shells get the line after 2 s.
        let (me, id) = (self.me.clone(), s.id.clone());
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(2));
            if let Some(core) = me.upgrade() {
                core.flush_pending(&id);
            }
        });
        Ok(())
    }

    fn flush_pending(&self, id: &str) {
        let line = self.inner.lock().unwrap().live.get_mut(id).and_then(|l| {
            let line = l.pending.take();
            if line.is_some() {
                l.typed_at = Some(Instant::now()); // only when something was actually typed
            }
            line
        });
        if let Some(line) = line {
            let _ = self.write(id, format!("{line}\r").as_bytes());
        }
    }

    /// Claude asks before trusting a new folder and fires no hooks until it is answered.
    fn on_output(&self, id: &str, text: &str) {
        let status = self.status(id);
        if status == Some(Status::Starting) && plain(text).contains("trust this folder") {
            self.set_status(id, Status::NeedsYou);
        }
    }

    /// A terminal's output stream ended: the terminal ended, or the stream or the helper did.
    fn on_stream_end(&self, id: &str) {
        if self.quitting.load(Ordering::SeqCst) {
            return;
        }
        match self.ptys.probe(id) {
            Probe::Gone => self.on_exit(id),
            Probe::Running => {
                let (out, exit) = self.listeners(id);
                if self.ptys.attach(id, out, exit).is_err() {
                    self.on_exit(id);
                }
            }
            // The helper died (crash, killed): the row resumes on select, as after a reboot.
            Probe::NoHelper => {
                if let Some(l) = self.inner.lock().unwrap().live.get_mut(id) {
                    l.claude_pid = None;
                }
                self.set_status(id, Status::Asleep);
            }
        }
    }

    /// The shell exited.
    fn on_exit(&self, id: &str) {
        if self.quitting.load(Ordering::SeqCst) {
            return; // quitting: keep rows so they come back asleep
        }
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if let Some(l) = inner.live.get_mut(id) {
                l.claude_pid = None;
            }
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                s.exited = true;
                s.claude_running = false;
            }
            self.save(inner);
        }
        self.set_status(id, Status::Exited);
    }

    /// Claude ended (SessionEnd) or its pid vanished: back to the plain shell.
    fn claude_gone(&self, id: &str) {
        // A dying helper hangs up on Claude too: that SessionEnd isn't the user ending it.
        if self.status(id) == Some(Status::Asleep) || self.ptys.probe(id) == Probe::NoHelper {
            return;
        }
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if let Some(l) = inner.live.get_mut(id) {
                l.claude_pid = None;
            }
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                s.claude_running = false;
                s.note = None;
            }
            self.save(inner);
        }
        self.set_status(id, Status::Shell);
    }

    /// Claude again in its terminal on the same conversation: stopped, then resumed.
    pub fn restart_claude(&self, id: &str) -> Result<(), String> {
        let s = self.session(id).ok_or("no such session")?;
        let pid = self.inner.lock().unwrap().live.get(id).and_then(|l| l.claude_pid);
        let pid = pid.filter(|_| self.claude_in_foreground(id)).ok_or("Claude isn't running in this terminal.")?;
        if s.claude_id.is_empty() || !past::saved(&self.projects(), &s.claude_id) {
            return Err("Nothing to come back to yet: send Claude a prompt first.".into());
        }
        let line = integration::resume_cmd(&s.claude_id);
        // no longer this row's Claude: neither reaped as gone nor restarted twice
        if let Some(l) = self.inner.lock().unwrap().live.get_mut(id) {
            l.claude_pid = None;
            l.typed_at = Some(Instant::now());
        }
        self.set_status(id, Status::Starting);
        let (me, id) = (self.me.clone(), id.to_string());
        thread::spawn(move || {
            // Claude saves as it goes and ends cleanly on SIGTERM (in about half a second)
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            let end = Instant::now() + Duration::from_secs(10);
            while alive(pid) && Instant::now() < end {
                thread::sleep(Duration::from_millis(100));
            }
            if alive(pid) {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                thread::sleep(Duration::from_millis(300));
            }
            // typed once Claude is gone, so the shell reads it, not Claude
            if let Some(core) = me.upgrade() {
                let _ = core.ptys.write(&id, format!("{line}\r").as_bytes());
            }
        });
        Ok(())
    }

    /// Flags each Claude older than the one installed now, and restarts the ones left idle.
    fn check_versions(&self) {
        let Some(installed) = self.setup.claude.as_ref().and_then(|c| c.canonicalize().ok()) else { return };
        let now = state::now_ms();
        let (mut changed, mut due) = (vec![], vec![]);
        {
            let mut inner = self.inner.lock().unwrap();
            for (id, l) in inner.live.iter_mut() {
                let outdated = l.claude_pid.and_then(exe_of).is_some_and(|exe| older(&exe, &installed));
                if outdated != l.outdated {
                    l.outdated = outdated;
                    changed.push(id.clone());
                }
                if restart_due(l, outdated, now) {
                    due.push(id.clone());
                }
            }
        }
        for id in changed {
            if let Some(v) = self.view(&id) {
                self.sink.session_changed(v);
            }
        }
        for id in due {
            let _ = self.restart_claude(&id);
        }
    }

    fn reap_claude(&self) {
        if self.quitting.load(Ordering::SeqCst) {
            return; // shells are being killed on purpose: keep claudeRunning for the relaunch
        }
        let dead: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            inner
                .live
                .iter()
                .filter(|(_, l)| l.claude_pid.is_some_and(|p| !alive(p)))
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in dead {
            self.claude_gone(&id);
        }
    }

    /// Opens an asleep row (shell, plus `claude --resume` if Claude was running in it).
    pub fn select(&self, id: &str) -> Result<View, String> {
        if self.claim_asleep(id) {
            let s = self.session(id).ok_or("no such session")?;
            // bring back its Claude, if it had one
            let line = (s.claude_running && !s.claude_id.is_empty()).then(|| {
                let saved = past::saved(&self.projects(), &s.claude_id).then_some(s.claude_id.as_str());
                integration::reopen_cmd(&s.workdir, saved)
            });
            self.spawn(&s, line)?;
        }
        self.view(id).ok_or_else(|| "no such session".into())
    }

    /// Asleep → Shell in one critical section; only the caller that wins opens it.
    fn claim_asleep(&self, id: &str) -> bool {
        let v = {
            let mut inner = self.inner.lock().unwrap();
            match inner.live.get_mut(id) {
                Some(l) if l.status == Status::Asleep => {
                    l.status = Status::Shell;
                    l.since = state::now_ms();
                }
                _ => return false,
            }
            view(&inner, id)
        };
        if let Some(v) = v {
            self.sink.session_changed(v);
        }
        true
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), String> {
        self.ptys.write(id, data).map_err(|e| e.to_string())?;
        if let Some(l) = self.inner.lock().unwrap().live.get_mut(id) {
            l.input_at = state::now_ms();
        }
        // Esc interrupts Claude's turn or declines its question, and Claude fires no hook
        // for either: without this the row would stay "working" or "needs you".
        if data == b"\x1b"
            && matches!(self.status(id), Some(Status::Working | Status::NeedsYou))
            && self.claude_in_foreground(id)
        {
            self.set_status(id, Status::Idle);
        }
        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        *self.size.lock().unwrap() = (cols, rows);
        self.ptys.resize(id, cols, rows);
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<View, String> {
        let name = state::display_name(name).ok_or("a name can't be empty")?;
        {
            let mut inner = self.inner.lock().unwrap();
            let taken: Vec<String> =
                inner.state.sessions.iter().filter(|s| s.id != id).map(|s| s.name.clone()).collect();
            let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
            let unique = state::unique_display(&name, &taken);
            let s = inner.state.sessions.iter_mut().find(|s| s.id == id).ok_or("no such session")?;
            s.name = unique;
            self.save(&inner);
        }
        let v = self.view(id).ok_or("no such session")?;
        self.sink.session_changed(v.clone());
        Ok(v)
    }

    pub fn close(&self, id: &str) {
        self.ptys.kill(id);
        let _ = std::fs::remove_file(integration::prompt_file(&self.setup.paths, id));
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(s) = inner.state.sessions.iter().find(|s| s.id == id) {
                self.closed.lock().unwrap().push(s.clone());
            }
            inner.state.sessions.retain(|s| s.id != id);
            inner.live.remove(id);
            self.save(&inner);
        }
        self.emit_all();
    }

    /// The last session closed, back in a new terminal: its name, its folder, and the Claude
    /// conversation it had.
    pub fn reopen_closed(&self) -> Result<View, String> {
        let s = self.closed.lock().unwrap().pop().ok_or("Nothing closed to bring back.")?;
        let dir = [&s.workdir, &s.repo].into_iter().find(|d| Path::new(d).is_dir()).cloned();
        let dir = dir.ok_or_else(|| format!("{}'s folder is gone.", s.name))?;
        let line = (s.claude_running && past::saved(&self.projects(), &s.claude_id)).then(|| integration::resume_cmd(&s.claude_id));
        let parent = s.parent.filter(|p| self.session(p).is_some());
        let req = NewSession { prompt: String::new(), repo: dir, worktree: false, name: Some(s.name), parent };
        self.create(req, line)
    }

    pub fn stop(&self, target: &str) -> Result<(), String> {
        let id = self.resolve(target).ok_or_else(|| format!("no session named {target}"))?;
        self.ptys.kill(&id);
        Ok(())
    }

    /// Claude has the terminal: its pid leads the foreground process group. Status alone can
    /// be stale (Ctrl-Z, a crash between hooks), and then a paste would run in the shell.
    fn claude_in_foreground(&self, id: &str) -> bool {
        let pid = self.inner.lock().unwrap().live.get(id).and_then(|l| l.claude_pid);
        pid.is_some_and(|p| self.ptys.foreground(id) == Some(p as i32))
    }

    /// A claude command is queued, or was typed moments ago and hasn't reported in yet.
    fn claude_on_its_way(&self, id: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.live.get(id).is_some_and(|l| {
            l.pending.is_some()
                || l.status == Status::Starting
                || l.typed_at.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) && l.claude_pid.is_none()
        })
    }

    pub fn send(&self, target: &str, text: &str, from: Option<String>) -> Result<(), String> {
        let id = self.resolve(target).ok_or_else(|| format!("no session named {target}"))?;
        let prefix = from
            .and_then(|f| self.session(&f))
            .map(|s| format!("[from {}] ", s.name))
            .unwrap_or_default();
        self.paste(&id, target, &format!("{prefix}{text}"))
    }

    /// Types `text` into the Claude running in a session as one paste, and submits it: waking the
    /// session if it's asleep, and waiting for a Claude that's starting.
    fn paste(&self, id: &str, target: &str, text: &str) -> Result<(), String> {
        let id = id.to_string();
        let resumable = self.session(&id).is_some_and(|s| s.claude_running);
        if self.status(&id) == Some(Status::Asleep) && resumable {
            self.select(&id)?;
        }
        let end = Instant::now() + Duration::from_secs(15);
        while !self.claude_in_foreground(&id) && self.claude_on_its_way(&id) && Instant::now() < end {
            thread::sleep(Duration::from_millis(100));
        }
        if !self.claude_in_foreground(&id) {
            return Err(format!("{target} isn't running Claude"));
        }
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend(text.as_bytes());
        bytes.extend(b"\x1b[201~");
        self.write(&id, &bytes)?;
        thread::sleep(Duration::from_millis(50));
        self.write(&id, b"\r")
    }

    pub fn list_past(&self, repo: &str) -> Vec<Past> {
        let open: HashSet<String> =
            self.inner.lock().unwrap().state.sessions.iter().map(|s| s.claude_id.clone()).collect();
        past::list_past(&self.projects(), repo, &open)
    }

    fn projects(&self) -> PathBuf {
        let home = self.setup.env.get("HOME").cloned().unwrap_or_default();
        Path::new(&home).join(".claude/projects")
    }

    pub fn resume_past(&self, repo: &str, p: Past) -> Result<View, String> {
        let id = {
            let mut inner = self.inner.lock().unwrap();
            let taken: Vec<&str> = inner.state.sessions.iter().map(|s| s.name.as_str()).collect();
            let s = Session {
                id: uuid::Uuid::new_v4().to_string(),
                claude_id: p.claude_id,
                name: state::unique_name(&state::slug(&p.title), &taken),
                repo: repo.into(),
                workdir: p.workdir,
                parent: None,
                created_at: state::now_ms(),
                exited: false,
                claude_running: true,
                note: None,
                turn_base: None,
                claude_status: None,
            };
            inner.live.insert(s.id.clone(), live(Status::Asleep));
            inner.state.sessions.push(s.clone());
            self.save(&inner);
            s.id
        };
        self.emit_all();
        self.select(&id)
    }

    /// The app quits: stop listening, and leave every session running in the helper.
    pub fn detach(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        self.ptys.detach();
    }

    /// The app is quitting: its sessions were already kept or ended.
    pub fn leaving(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    /// End every session and the helper (Stop All Sessions and Quit; tests).
    pub fn shutdown(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        self.ptys.stop_all();
    }

    pub fn handle(&self, req: Request) -> Response {
        let done = |r: Result<(), String>| r.map_or_else(Response::err, |_| Response::ok());
        match req {
            Request::New { prompt, cwd, worktree, name, parent } => {
                let req = NewSession { prompt, repo: cwd, worktree: worktree.is_some(), name: name.or(worktree), parent };
                match self.new_session(req) {
                    Ok(v) => Response { id: Some(v.id), name: Some(v.name), ..Response::ok() },
                    Err(e) => Response::err(e),
                }
            }
            Request::Ls => {
                let rows = self
                    .views()
                    .into_iter()
                    .map(|v| LsRow { id: v.id, name: v.name, status: v.status, repo: v.repo, parent: v.parent })
                    .collect();
                Response { sessions: Some(rows), ..Response::ok() }
            }
            Request::Send { target, text, from } => done(self.send(&target, &text, from)),
            Request::Stop { target } => done(self.stop(&target)),
            Request::Say { from, text } => match from {
                Some(from) => done(self.say(&from, &text)),
                None => Response::err("petramux say works inside a petramux session"),
            },
            Request::Chat { from: None, .. } => Response::err("petramux chat works inside a petramux session"),
            Request::Chat { from: Some(from), tail, wait, pid } => {
                if wait {
                    let Some(pid) = pid.filter(|&p| p > 0) else { return Response::err("waiting needs the waiting command's pid") };
                    if self.chat_of(&from).is_none() {
                        return Response::err("this session isn't in a workspace");
                    }
                    if let Err(e) = self.listen(&from, pid) {
                        return Response::err(e);
                    }
                }
                match self.read_chat(&from, tail) {
                    Ok((c, msgs, unread)) => Response { name: Some(c.name), messages: Some(msgs), unread: Some(unread), ..Response::ok() },
                    Err(e) => Response::err(e),
                }
            }
            Request::Hook { session, event, payload } => Response { hook: self.hook(&session, &event, &payload), ..Response::ok() },
        }
    }

    /// What happened in a session, from its hooks. The answer, if any, is for Claude: chat
    /// messages that arrived while it was working, handed over at its next tool call, or when it's
    /// about to stop (it then carries on with them rather than stopping).
    fn hook(&self, id: &str, event: &str, payload: &Value) -> Option<Value> {
        if self.quitting.load(Ordering::SeqCst) {
            return None; // Claude's own shutdown (SessionEnd on SIGHUP) must not clear claudeRunning
        }
        match event {
            "ShellReady" => {
                self.flush_pending(id);
                return None;
            }
            // `/clear` ends one conversation and starts the next in the same process.
            "SessionEnd" if payload["reason"] == "clear" => return None,
            "SessionEnd" => {
                self.claude_gone(id);
                return None;
            }
            "PostToolUse" => {
                if let Some(text) = self.take_inbox(id) {
                    let context = format!("New in the workspace chat (act only on what addresses you):\n\n{text}");
                    return Some(serde_json::json!({ "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": context } }));
                }
            }
            "Stop" => {
                if let Some(text) = self.take_inbox(id) {
                    let reason = format!("Before you stop: new in the workspace chat (act only on what addresses you):\n\n{text}");
                    return Some(serde_json::json!({ "decision": "block", "reason": reason })); // still working
                }
                // once per stop: never twice in a row, whatever Claude makes of it
                if payload["stop_hook_active"] != true && self.should_listen(id) {
                    return Some(serde_json::json!({ "decision": "block", "reason": LISTEN }));
                }
            }
            _ => {}
        }
        let mut note_changed = false;
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if !inner.state.sessions.iter().any(|s| s.id == id) {
                return None;
            }
            match event {
                "ClaudeStart" => {
                    let _ = std::fs::remove_file(integration::prompt_file(&self.setup.paths, id)); // already read
                    if let Some(l) = inner.live.get_mut(id) {
                        l.claude_pid = payload["pid"].as_u64().map(|p| p as u32);
                        l.typed_at = None; // reported in: no longer "on its way"
                    }
                }
                "UserPromptSubmit" => {
                    if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                        s.turn_base = payload["roost_base"].as_str().map(String::from);
                    }
                }
                "SessionStart" => {
                    if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                        if let Some(c) = payload["session_id"].as_str() {
                            s.claude_id = c.into();
                        }
                        if let Some(c) = payload["cwd"].as_str() {
                            s.workdir = c.into();
                        }
                        s.claude_running = true;
                    }
                    self.save(inner);
                }
                _ => {}
            }
            if let Some(note) = state::note_for(event, payload) {
                if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                    if s.note != note {
                        s.note = note;
                        note_changed = true;
                    }
                }
            }
            if note_changed || event == "UserPromptSubmit" {
                self.save(inner);
            }
        }
        let emitted = state::status_for(event, payload).is_some_and(|st| self.set_status(id, st));
        if note_changed && !emitted {
            if let Some(v) = self.view(id) {
                self.sink.session_changed(v);
            }
        }
        None
    }

    /// A session's waiting chat messages, taken (each is handed over once).
    fn take_inbox(&self, id: &str) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        let waiting = std::mem::take(&mut inner.live.get_mut(id)?.inbox);
        let text = waiting.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join("\n\n");
        // Claude Code cuts a hook's text at 10,000 characters (into a file it isn't told to read)
        const MAX: usize = 9_000;
        let text = if text.chars().count() > MAX {
            let cut: String = text.chars().take(MAX).collect();
            format!("{cut}\n… (cut short: `petramux chat` has all of it)")
        } else {
            text
        };
        (!text.is_empty()).then_some(text)
    }
}

const LISTEN: &str = "Before you stop: you share a workspace chat with other sessions. Start `petramux chat --wait` as a background command (run_in_background) so you hear from them: it ends when a message @mentions you, and that wakes you to read it. Then stop.";

/// Terminal text with CSI escapes (Claude positions words with them) turned into
/// spaces and whitespace collapsed, so phrases can be matched.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() || n == '~' {
                    break;
                }
            }
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn view(inner: &Inner, id: &str) -> Option<View> {
    let s = inner.state.sessions.iter().find(|s| s.id == id)?;
    let l = inner.live.get(id)?;
    Some(View {
        id: s.id.clone(),
        name: s.name.clone(),
        repo: s.repo.clone(),
        workdir: s.workdir.clone(),
        parent: s.parent.clone(),
        status: l.status,
        since: l.since,
        created_at: s.created_at,
        note: s.note.clone(),
        outdated: l.outdated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claude_is_out_of_date_when_a_newer_version_sits_beside_it() {
        let v = |n: &str| PathBuf::from(format!("/u/.local/share/claude/versions/{n}"));
        assert!(older(&v("2.1.296"), &v("2.1.300")));
        assert!(!older(&v("2.1.300"), &v("2.1.300")));
        // a script install runs under node: never taken as out of date
        assert!(!older(Path::new("/opt/homebrew/bin/node"), Path::new("/opt/homebrew/lib/node_modules/cli.js")));
        let canon = |p: Option<PathBuf>| p.and_then(|p| p.canonicalize().ok());
        assert_eq!(canon(exe_of(std::process::id())), canon(std::env::current_exe().ok()));
    }

    #[test]
    fn an_out_of_date_claude_restarts_itself_only_when_left_idle_untouched() {
        let now = 1_000_000;
        let idle = |since: u64, input_at: u64| Live { since, input_at, claude_pid: Some(1), ..live(Status::Idle) };
        assert!(restart_due(&idle(now - 120_000, 0), true, now));
        assert!(!restart_due(&idle(now - 120_000, 0), false, now)); // up to date
        assert!(!restart_due(&idle(now - 10_000, 0), true, now)); // a turn just ended: you may be about to type
        assert!(!restart_due(&idle(now - 120_000, now - 60_000), true, now)); // typed since: maybe a draft
        assert!(!restart_due(&idle(now - 120_000, now - 120_000), true, now)); // kept from before launch: unknown
        assert!(!restart_due(&Live { since: now - 120_000, claude_pid: Some(1), ..live(Status::Working) }, true, now));
    }

    #[test]
    fn plain_text_drops_cursor_escapes() {
        assert_eq!(plain("\x1b[4GYes,\x1b[9GI\x1b[11Gtrust\x1b[17Gthis\r\n"), "Yes, I trust this");
    }
}
