use crate::paths::Paths;
use crate::past::{self, Past};
use crate::pty::{self, Launch, PtyManager, Spawn};
use crate::shellenv::{self, Env};
use crate::sock::{Handler, LsRow, Request, Response};
use crate::state::{self, Session, Status};
use serde::Serialize;
use serde_json::json;
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
    pub worktree: bool,
    pub parent: Option<String>,
    pub status: Status,
    pub since: u64,
    pub created_at: u64,
    pub resume_failed: bool,
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

struct Live {
    status: Status,
    since: u64,
    spawned_at: Option<Instant>,
    resumed: bool,
    resume_failed: bool,
}

struct Inner {
    state: state::State,
    live: HashMap<String, Live>,
}

pub struct Core {
    inner: Mutex<Inner>,
    paths: Paths,
    env: Env,
    claude: Option<PathBuf>,
    ptys: PtyManager,
    sink: Arc<dyn Sink>,
    size: Mutex<(u16, u16)>,
    quitting: AtomicBool,
    me: Weak<Core>,
}

fn live(status: Status) -> Live {
    Live { status, since: state::now_ms(), spawned_at: None, resumed: false, resume_failed: false }
}

impl Core {
    pub fn new(paths: Paths, env: Env, claude: Option<PathBuf>, sink: Arc<dyn Sink>) -> Arc<Core> {
        let state = state::load(&paths.state());
        let live = state
            .sessions
            .iter()
            .map(|s| (s.id.clone(), live(if s.exited { Status::Exited } else { Status::Asleep })))
            .collect();
        Arc::new_cyclic(|me| Core {
            inner: Mutex::new(Inner { state, live }),
            paths,
            env,
            claude,
            ptys: PtyManager::default(),
            sink,
            size: Mutex::new((120, 36)),
            quitting: AtomicBool::new(false),
            me: me.clone(),
        })
    }

    pub fn handler(self: &Arc<Self>) -> Handler {
        let me = self.clone();
        Arc::new(move |req| me.handle(req))
    }

    pub fn claude_found(&self) -> bool {
        self.claude.is_some()
    }

    pub fn recent_repos(&self) -> Vec<String> {
        self.inner.lock().unwrap().state.recent_repos.clone()
    }

    pub fn views(&self) -> Vec<View> {
        let inner = self.inner.lock().unwrap();
        inner.state.sessions.iter().filter_map(|s| view(&inner, &s.id)).collect()
    }

    fn save(&self, inner: &Inner) {
        if let Err(e) = state::save(&self.paths.state(), &inner.state) {
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
            if l.status == status || (l.status == Status::Exited && status != Status::Starting) {
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

    /// Resolves a name or id to an id.
    fn resolve(&self, target: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner.state.sessions.iter().find(|s| s.id == target || s.name == target).map(|s| s.id.clone())
    }

    fn session(&self, id: &str) -> Option<Session> {
        self.inner.lock().unwrap().state.sessions.iter().find(|s| s.id == id).cloned()
    }

    fn status(&self, id: &str) -> Option<Status> {
        self.inner.lock().unwrap().live.get(id).map(|l| l.status)
    }

    pub fn new_session(&self, req: NewSession) -> Result<View, String> {
        if self.claude.is_none() {
            return Err("Claude Code not found. Install it, or set ROOST_CLAUDE_BIN.".into());
        }
        // portable-pty silently falls back to $HOME for a missing cwd.
        if !Path::new(&req.repo).is_dir() {
            return Err(format!("no such folder: {}", req.repo));
        }
        let session = {
            let mut inner = self.inner.lock().unwrap();
            let taken: Vec<&str> = inner.state.sessions.iter().map(|s| s.name.as_str()).collect();
            let base = state::slug(req.name.as_deref().unwrap_or(&req.prompt));
            let s = Session {
                id: uuid::Uuid::new_v4().to_string(),
                claude_id: uuid::Uuid::new_v4().to_string(),
                name: state::unique_name(&base, &taken),
                repo: req.repo.clone(),
                workdir: req.repo.clone(),
                claude_running: false,
                parent: req.parent,
                created_at: state::now_ms(),
                exited: false,
            };
            inner.state.sessions.push(s.clone());
            inner.live.insert(s.id.clone(), live(Status::Starting));
            let recent = &mut inner.state.recent_repos;
            recent.retain(|r| r != &req.repo);
            recent.insert(0, req.repo.clone());
            recent.truncate(10);
            self.save(&inner);
            s
        };
        self.emit_all();
        self.spawn(&session, Launch::New { prompt: &req.prompt, worktree: req.worktree })?;
        self.view(&session.id).ok_or_else(|| "session vanished".into())
    }

    fn view(&self, id: &str) -> Option<View> {
        view(&self.inner.lock().unwrap(), id)
    }

    fn spawn(&self, s: &Session, launch: Launch) -> Result<(), String> {
        let claude = self.claude.clone().ok_or("Claude Code not found")?;
        let (cols, rows) = *self.size.lock().unwrap();
        let resumed = matches!(launch, Launch::Resume);
        let cwd = if resumed { &s.workdir } else { &s.repo };
        let spec = Spawn {
            program: claude,
            args: pty::claude_args(&s.claude_id, &s.name, &self.paths.hooks(), launch),
            cwd: PathBuf::from(cwd),
            env: shellenv::child_env(&self.env, &s.id, &self.paths.sock()),
            cols,
            rows,
        };
        if let Some(l) = self.inner.lock().unwrap().live.get_mut(&s.id) {
            l.spawned_at = Some(Instant::now());
            l.resumed = resumed;
            l.resume_failed = false;
        }
        if !Path::new(cwd).is_dir() {
            self.on_exit(&s.id); // a resume into a deleted worktree fails here, not in $HOME
            return Ok(());
        }
        self.set_status(&s.id, Status::Starting);
        let sink = self.sink.clone();
        let out_id = s.id.clone();
        let exit_id = s.id.clone();
        let me = self.me.clone();
        let out_me = self.me.clone();
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
                if let Some(core) = me.upgrade() {
                    core.on_exit(&exit_id);
                }
            },
        );
        if let Err(e) = result {
            if e.kind() == io::ErrorKind::AlreadyExists {
                return Ok(()); // someone else already started it
            }
            self.on_exit(&s.id);
            return Err(format!("could not start claude: {e}"));
        }
        if self.session(&s.id).is_none() {
            self.ptys.kill(&s.id); // the row was closed while we were spawning
        }
        Ok(())
    }

    /// Claude asks before trusting a new folder and fires no hooks until it is answered.
    fn on_output(&self, id: &str, text: &str) {
        if self.status(id) == Some(Status::Starting) && plain(text).contains("trust this folder") {
            self.set_status(id, Status::NeedsYou);
        }
    }

    fn on_exit(&self, id: &str) {
        if self.quitting.load(Ordering::SeqCst) {
            return; // quitting: keep rows non-exited so they come back asleep
        }
        {
            let mut inner = self.inner.lock().unwrap();
            let Some(l) = inner.live.get_mut(id) else { return };
            l.resume_failed = l.resumed && l.spawned_at.is_some_and(|t| t.elapsed() < Duration::from_secs(3));
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                s.exited = true;
            }
            self.save(&inner);
        }
        self.set_status(id, Status::Exited);
    }

    pub fn select(&self, id: &str) -> Result<View, String> {
        if self.claim_asleep(id) {
            let s = self.session(id).ok_or("no such session")?;
            self.spawn(&s, Launch::Resume)?;
        }
        self.view(id).ok_or_else(|| "no such session".into())
    }

    /// Asleep → Starting in one critical section; only the caller that wins resumes it.
    fn claim_asleep(&self, id: &str) -> bool {
        let v = {
            let mut inner = self.inner.lock().unwrap();
            match inner.live.get_mut(id) {
                Some(l) if l.status == Status::Asleep => {
                    l.status = Status::Starting;
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
        {
            let mut inner = self.inner.lock().unwrap();
            let taken: Vec<String> =
                inner.state.sessions.iter().filter(|s| s.id != id).map(|s| s.name.clone()).collect();
            let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
            let unique = state::unique_name(&state::slug(name), &taken);
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

    pub fn send(&self, target: &str, text: &str, from: Option<String>) -> Result<(), String> {
        let id = self.resolve(target).ok_or_else(|| format!("no session named {target}"))?;
        if self.status(&id) == Some(Status::Asleep) {
            self.select(&id)?;
            let end = Instant::now() + Duration::from_secs(15);
            while self.status(&id) == Some(Status::Starting) && Instant::now() < end {
                thread::sleep(Duration::from_millis(100));
            }
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

    pub fn start_fresh(&self, id: &str) -> Result<View, String> {
        let old = self.session(id).ok_or("no such session")?;
        let dir = if Path::new(&old.workdir).is_dir() { old.workdir.clone() } else { old.repo.clone() };
        self.close(id);
        self.new_session(NewSession { prompt: String::new(), repo: dir, worktree: false, name: Some(old.name), parent: old.parent })
    }

    pub fn list_past(&self, repo: &str) -> Vec<Past> {
        let open: HashSet<String> =
            self.inner.lock().unwrap().state.sessions.iter().map(|s| s.claude_id.clone()).collect();
        let home = self.env.get("HOME").cloned().unwrap_or_default();
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
                claude_running: false,
                parent: None,
                created_at: state::now_ms(),
                exited: false,
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

    fn hook(&self, id: &str, event: &str, payload: &serde_json::Value) {
        {
            let mut inner = self.inner.lock().unwrap();
            let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) else { return };
            if event == "SessionStart" {
                if let Some(c) = payload["session_id"].as_str() {
                    s.claude_id = c.into();
                }
                if let Some(c) = payload["cwd"].as_str() {
                    s.workdir = c.into();
                }
                self.save(&inner);
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
        worktree: false,
        parent: s.parent.clone(),
        status: l.status,
        since: l.since,
        created_at: s.created_at,
        resume_failed: l.resume_failed,
    })
}

/// The per-session Claude Code settings: one `roost hook <Event>` command per event.
pub fn hooks_json(exe: &Path) -> serde_json::Value {
    let quoted = format!("'{}'", exe.display().to_string().replace('\'', r"'\''"));
    let entry = |matcher: Option<&str>, event: &str| {
        let mut e = json!({"hooks": [{"type": "command", "command": format!("{quoted} hook {event}")}]});
        if let Some(m) = matcher {
            e["matcher"] = json!(m);
        }
        json!([e])
    };
    json!({"hooks": {
        "SessionStart": entry(None, "SessionStart"),
        "UserPromptSubmit": entry(None, "UserPromptSubmit"),
        "PostToolUse": entry(Some("*"), "PostToolUse"),
        "Notification": entry(Some("permission_prompt|elicitation_dialog"), "Notification"),
        "Stop": entry(None, "Stop"),
    }})
}

pub fn write_hooks(paths: &Paths, exe: &Path) -> io::Result<()> {
    std::fs::create_dir_all(&paths.dir)?;
    std::fs::write(paths.hooks(), serde_json::to_vec_pretty(&hooks_json(exe))?)
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
    fn hooks_file_registers_every_event_with_quoted_binary() {
        let v = hooks_json(Path::new("/Apps/It's Roost/roost"));
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 5);
        assert_eq!(hooks["Notification"][0]["matcher"], "permission_prompt|elicitation_dialog");
        assert_eq!(hooks["PostToolUse"][0]["matcher"], "*");
        assert_eq!(hooks["Stop"][0]["hooks"][0]["command"], r"'/Apps/It'\''s Roost/roost' hook Stop");
    }

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
