use crate::integration;
use crate::past::{self, Past};
use crate::paths::Paths;
use crate::pty::{PtyManager, Spawn};
use crate::shellenv::Env;
use crate::sock::{Handler, LsRow, Request, Response};
use crate::state::{self, Session, Status};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
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
}

struct Inner {
    state: state::State,
    live: HashMap<String, Live>,
}

pub struct Core {
    inner: Mutex<Inner>,
    setup: Setup,
    ptys: PtyManager,
    sink: Arc<dyn Sink>,
    size: Mutex<(u16, u16)>,
    quitting: AtomicBool,
    me: Weak<Core>,
}

fn live(status: Status) -> Live {
    Live { status, since: state::now_ms(), claude_pid: None, pending: None, typed_at: None }
}

fn alive(pid: u32) -> bool {
    let signalled = unsafe { libc::kill(pid as i32, 0) } == 0;
    signalled || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

impl Core {
    pub fn new(setup: Setup, sink: Arc<dyn Sink>) -> Arc<Core> {
        if let Err(e) = integration::install(&setup.paths, &setup.roost_bin) {
            eprintln!("roost: could not install shell integration: {e}");
        }
        let state = state::load(&setup.paths.state());
        let live = state
            .sessions
            .iter()
            .map(|s| (s.id.clone(), live(if s.exited { Status::Exited } else { Status::Asleep })))
            .collect();
        let core = Arc::new_cyclic(|me| Core {
            inner: Mutex::new(Inner { state, live }),
            setup,
            ptys: PtyManager::default(),
            sink,
            size: Mutex::new((120, 36)),
            quitting: AtomicBool::new(false),
            me: me.clone(),
        });
        // Claude killed without SessionEnd (Ctrl-C, crash) still returns its row to the shell.
        let weak = Arc::downgrade(&core);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(2));
            let Some(core) = weak.upgrade() else { break };
            core.reap_claude();
        });
        core
    }

    pub fn handler(self: &Arc<Self>) -> Handler {
        let me = self.clone();
        Arc::new(move |req| me.handle(req))
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

    pub fn save_layout(&self, layout: Option<Value>, focus: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.state.layout = layout;
        inner.state.focus = focus;
        self.save(&inner);
    }

    fn save(&self, inner: &Inner) {
        if let Err(e) = state::save(&self.setup.paths.state(), &inner.state) {
            eprintln!("roost: could not save state: {e}");
        }
    }

    fn emit_all(&self) {
        self.sink.sessions_changed(self.views());
    }

    fn set_status(&self, id: &str, status: Status) {
        let (v, attention) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(l) = inner.live.get_mut(id) else { return };
            if l.status == status || l.status == Status::Exited {
                return;
            }
            let prev = l.status;
            l.status = status;
            l.since = state::now_ms();
            let attention = match (prev, status) {
                (_, Status::NeedsYou) => Some(Attention::NeedsYou),
                (Status::Working, Status::Idle) => Some(Attention::Done),
                _ => None,
            };
            (view(&inner, id), attention)
        };
        if let Some(v) = v {
            if let Some(a) = attention {
                self.sink.attention(&v, a);
            }
            self.sink.session_changed(v);
        }
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
        let line = (!prompt.is_empty())
            .then(|| integration::claude_cmd(&prompt_file, req.worktree.then_some(session.name.as_str())));
        self.spawn(&session, line)?;
        self.view(&session.id).ok_or_else(|| "session vanished".into())
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
        let sink = self.sink.clone();
        let (out_id, exit_id) = (s.id.clone(), s.id.clone());
        let (out_me, exit_me) = (self.me.clone(), self.me.clone());
        let result = self.ptys.spawn(
            &s.id,
            spec,
            move |text| {
                if let Some(core) = out_me.upgrade() {
                    core.on_output(&out_id, &text);
                }
                sink.output(&out_id, text)
            },
            move || {
                if let Some(core) = exit_me.upgrade() {
                    core.on_exit(&exit_id);
                }
            },
        );
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
        if self.status(id) == Some(Status::Starting) && plain(text).contains("trust this folder") {
            self.set_status(id, Status::NeedsYou);
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
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if let Some(l) = inner.live.get_mut(id) {
                l.claude_pid = None;
            }
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                s.claude_running = false;
            }
            self.save(inner);
        }
        self.set_status(id, Status::Shell);
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
            let line = (s.claude_running && !s.claude_id.is_empty())
                .then(|| integration::resume_cmd(&s.workdir, &s.claude_id));
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
        self.ptys.write(id, data).map_err(|e| e.to_string())
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
            inner.state.sessions.retain(|s| s.id != id);
            inner.live.remove(id);
            self.save(&inner);
        }
        self.emit_all();
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
        let prefix = from
            .and_then(|f| self.session(&f))
            .map(|s| format!("[from {}] ", s.name))
            .unwrap_or_default();
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend(prefix.as_bytes());
        bytes.extend(text.as_bytes());
        bytes.extend(b"\x1b[201~");
        self.write(&id, &bytes)?;
        thread::sleep(Duration::from_millis(50));
        self.write(&id, b"\r")
    }

    pub fn list_past(&self, repo: &str) -> Vec<Past> {
        let open: HashSet<String> =
            self.inner.lock().unwrap().state.sessions.iter().map(|s| s.claude_id.clone()).collect();
        let home = self.setup.env.get("HOME").cloned().unwrap_or_default();
        past::list_past(&Path::new(&home).join(".claude/projects"), repo, &open)
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
            };
            inner.live.insert(s.id.clone(), live(Status::Asleep));
            inner.state.sessions.push(s.clone());
            self.save(&inner);
            s.id
        };
        self.emit_all();
        self.select(&id)
    }

    pub fn shutdown(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        self.ptys.kill_all();
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
            Request::Hook { session, event, payload } => {
                self.hook(&session, &event, &payload);
                Response::ok()
            }
        }
    }

    fn hook(&self, id: &str, event: &str, payload: &Value) {
        if self.quitting.load(Ordering::SeqCst) {
            return; // Claude's own shutdown (SessionEnd on SIGHUP) must not clear claudeRunning
        }
        match event {
            "ShellReady" => return self.flush_pending(id),
            // `/clear` ends one conversation and starts the next in the same process.
            "SessionEnd" if payload["reason"] == "clear" => return,
            "SessionEnd" => return self.claude_gone(id),
            _ => {}
        }
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if !inner.state.sessions.iter().any(|s| s.id == id) {
                return;
            }
            match event {
                "ClaudeStart" => {
                    let _ = std::fs::remove_file(integration::prompt_file(&self.setup.paths, id)); // already read
                    if let Some(l) = inner.live.get_mut(id) {
                        l.claude_pid = payload["pid"].as_u64().map(|p| p as u32);
                        l.typed_at = None; // reported in: no longer "on its way"
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
        }
        if let Some(st) = state::status_for(event, payload) {
            self.set_status(id, st);
        }
    }
}

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
    })
}

pub fn is_git_repo(path: &str) -> bool {
    std::process::Command::new("/usr/bin/git")
        .args(["-C", path, "rev-parse", "--is-inside-work-tree"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_drops_cursor_escapes() {
        assert_eq!(plain("\x1b[4GYes,\x1b[9GI\x1b[11Gtrust\x1b[17Gthis\r\n"), "Yes, I trust this");
    }

    #[test]
    fn detects_git_repos() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(&dir.path().display().to_string()));
        std::process::Command::new("/usr/bin/git").args(["init", "-q"]).current_dir(dir.path()).status().unwrap();
        assert!(is_git_repo(&dir.path().display().to_string()));
    }
}
