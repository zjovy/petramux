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
    pub note: Option<String>,
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
    /// The background jobs not open in a pane changed.
    fn jobs_changed(&self, jobs: Vec<state::Job>);
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
    /// The name in Claude's terminal title: a background job's, when the pane shows one.
    job: Option<String>,
    /// The status came from that job's record (hooks never reach petramux for those).
    by_job: bool,
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
    /// Every recent job no pane shows, as last seen (finished ones too, to tell when one
    /// finishes): None until the first look.
    jobs: Mutex<Option<Vec<state::Job>>>,
    me: Weak<Core>,
}

fn live(status: Status) -> Live {
    Live { status, since: state::now_ms(), claude_pid: None, pending: None, typed_at: None, job: None, by_job: false }
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
        let mut state = state::load(&setup.paths.state());
        // The helper keeps terminals running while the app is closed: those rows are live, not asleep.
        let ptys = PtyManager::new(setup.paths.pty_sock(), Some(setup.roost_bin.clone()));
        let running = ptys.running();
        let mut live_rows = HashMap::new();
        for s in &mut state.sessions {
            let term = running.iter().find(|t| t.id == s.id);
            // Claude still has the keyboard: something other than the shell leads the foreground
            // (Claude, or Claude's view of a background job).
            let claude = term
                .and_then(|t| t.fg.filter(|fg| Some(*fg as u32) != t.pid))
                .filter(|_| s.claude_running || s.job.is_some());
            let mut l = live(match (term, claude) {
                // a question still waits for you (no hook fires until it's answered); "working" comes
                // back from the next hook or the job's record, never from a stale save
                (Some(_), Some(_)) => s.claude_status.filter(|st| *st == Status::NeedsYou).unwrap_or(Status::Idle),
                (Some(_), None) => Status::Shell,
                (None, _) if s.exited => Status::Exited,
                (None, _) => Status::Asleep,
            });
            if term.is_some() {
                l.claude_pid = claude.map(|p| p as u32);
                l.job = s.job.clone().filter(|_| claude.is_some());
                // Claude ended while the app was closed
                s.claude_running &= claude.is_some();
                if claude.is_none() {
                    s.job = None;
                }
                s.exited = false;
            }
            live_rows.insert(s.id.clone(), l);
        }
        let core = Arc::new_cyclic(|me| Core {
            inner: Mutex::new(Inner { state, live: live_rows }),
            setup,
            ptys,
            sink,
            size: Mutex::new((120, 36)),
            quitting: AtomicBool::new(false),
            jobs: Mutex::new(None),
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
            core.sync_jobs();
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

    /// What changed in the session's folder: since the last prompt ("turn"), or since HEAD ("all").
    pub fn diff(&self, id: &str, scope: &str) -> Result<String, String> {
        let (dir, base) = {
            let inner = self.inner.lock().unwrap();
            let s = inner.state.sessions.iter().find(|s| s.id == id).ok_or("no such session")?;
            (s.workdir.clone(), s.turn_base.clone())
        };
        match (scope, base) {
            ("all", _) => crate::git::diff(Path::new(&dir), None),
            (_, Some(b)) => crate::git::diff(Path::new(&dir), Some(&b)),
            (_, None) => Err(crate::git::no_turn(Path::new(&dir))),
        }
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
        if s.claude_id.is_empty() || !past::saved(&self.projects(), &s.claude_id) {
            return Err("Nothing to fork yet: send Claude a prompt first.".into());
        }
        let req = NewSession {
            prompt: String::new(),
            repo: s.workdir.clone(),
            worktree: false,
            name: Some(format!("{} fork", s.name)),
            parent: Some(s.id.clone()),
        };
        self.create(req, Some(integration::fork_cmd(&s.claude_id)))
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
                job: None,
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
        // Claude's title names what the pane shows; a background job's status comes from its record.
        if text.contains("\x1b]") {
            if let Some(name) = state::title_name(text) {
                let mut inner = self.inner.lock().unwrap();
                if let Some(l) = inner.live.get_mut(id) {
                    l.job = Some(name.clone());
                }
                if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                    if s.job.as_deref() != Some(name.as_str()) {
                        s.job = Some(name);
                        self.save(&inner);
                    }
                }
            }
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
                l.job = None; // a job it showed goes back to the jobs list
                l.by_job = false;
            }
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                s.claude_running = false;
                s.note = None;
                s.job = None;
            }
            self.save(inner);
        }
        self.set_status(id, Status::Shell);
    }

    /// Rows showing a Claude background job take its status and summary from the job's record: the
    /// job runs in Claude's own background service, so its hooks never reach petramux.
    fn sync_jobs(&self) {
        if self.quitting.load(Ordering::SeqCst) {
            return;
        }
        let jobs = self.jobs_dir();
        let rows: Vec<(String, Option<String>, bool, Status)> = {
            let inner = self.inner.lock().unwrap();
            inner.live.iter().map(|(id, l)| (id.clone(), l.job.clone(), l.by_job, l.status)).collect()
        };
        for (id, name, by_job, status) in rows {
            if matches!(status, Status::Asleep | Status::Exited | Status::Shell | Status::Starting) {
                continue;
            }
            match name.and_then(|n| state::find_job(&jobs, &n)) {
                Some(job) => {
                    let note_changed = {
                        let mut inner = self.inner.lock().unwrap();
                        if let Some(l) = inner.live.get_mut(&id) {
                            l.by_job = true;
                        }
                        let s = inner.state.sessions.iter_mut().find(|s| s.id == id);
                        match s {
                            Some(s) if s.note != job.note && job.note.is_some() => {
                                s.note = job.note.clone();
                                true
                            }
                            _ => false,
                        }
                    };
                    if !self.set_status(&id, job.status) && note_changed {
                        if let Some(v) = self.view(&id) {
                            self.sink.session_changed(v);
                        }
                    }
                }
                // it was showing a job and isn't any more: the hooks have the pane again
                None if by_job => {
                    if let Some(l) = self.inner.lock().unwrap().live.get_mut(&id) {
                        l.by_job = false;
                    }
                    self.set_status(&id, Status::Idle);
                }
                None => {}
            }
        }
        self.sync_job_list(&jobs);
    }

    fn jobs_dir(&self) -> PathBuf {
        let home = self.setup.env.get("HOME").cloned().unwrap_or_default();
        Path::new(&home).join(".claude/jobs")
    }

    /// The jobs list: every recent job no pane shows. One that starts needing you, or finishes,
    /// gets the same notification a session would.
    fn sync_job_list(&self, dir: &Path) {
        // one at a time (the sync thread, open_job): a stale list must never land after a newer one
        let mut cache = self.jobs.lock().unwrap();
        // shown: a pane where Claude shows the job, or is about to (typed, not reported in yet)
        let shown: HashSet<String> = {
            let inner = self.inner.lock().unwrap();
            let showing = |l: &&Live| {
                !matches!(l.status, Status::Asleep | Status::Exited)
                    && (l.claude_pid.is_some() || l.pending.is_some() || l.typed_at.is_some_and(|t| t.elapsed() < Duration::from_secs(10)))
            };
            inner.live.values().filter(showing).filter_map(|l| l.job.clone()).collect()
        };
        let list: Vec<state::Job> = state::list_jobs(dir, state::now_ms())
            .into_iter()
            .filter(|j| !shown.contains(&j.name) && !shown.contains(&j.id))
            .collect();
        let prev = cache.replace(list.clone());
        if prev.as_ref() == Some(&list) {
            return;
        }
        for j in &list {
            let before = prev.as_ref().and_then(|p| p.iter().find(|p| p.id == j.id)).map(|p| p.status);
            let attention = match (before, j.status) {
                (Some(b), Status::NeedsYou) if b != Status::NeedsYou => Some(Attention::NeedsYou),
                (Some(Status::Working), Status::Idle) => Some(Attention::Done),
                _ => None,
            };
            if let Some(a) = attention {
                let v = View {
                    id: format!("job:{}", j.id),
                    name: j.name.clone(),
                    repo: j.cwd.clone(),
                    workdir: j.cwd.clone(),
                    parent: None,
                    status: j.status,
                    since: j.since,
                    created_at: j.since,
                    note: j.note.clone(),
                };
                self.sink.attention(&v, a);
            }
        }
        self.sink.jobs_changed(listed(&list));
    }

    /// The jobs list: those working or waiting on you (a finished job has nothing to do).
    pub fn jobs(&self) -> Vec<state::Job> {
        listed(self.jobs.lock().unwrap().as_deref().unwrap_or_default())
    }

    /// A listed job, open in a new pane: `claude attach` in its folder. The pane takes the job's
    /// status from then on, and the job leaves the list.
    pub fn open_job(&self, id: &str) -> Result<View, String> {
        let job = self.jobs().into_iter().find(|j| j.id == id).ok_or("that job is gone")?;
        let repo = if Path::new(&job.cwd).is_dir() { job.cwd.clone() } else { String::new() };
        let req = NewSession { prompt: String::new(), repo, worktree: false, name: Some(job.name.clone()), parent: None };
        let v = self.create(req, Some(integration::attach_cmd(&job.id)))?;
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(l) = inner.live.get_mut(&v.id) {
                l.job = Some(job.name.clone());
            }
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == v.id) {
                s.job = Some(job.name.clone());
            }
            self.save(&inner);
        }
        self.sync_job_list(&self.jobs_dir());
        Ok(v)
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
            // it showed a background job: show it again; else bring back its Claude, if it had one
            let job = s.job.clone().filter(|_| !s.claude_running);
            let line = match &job {
                Some(job) => Some(integration::attach_cmd(job)),
                None => (s.claude_running && !s.claude_id.is_empty()).then(|| {
                    let saved = past::saved(&self.projects(), &s.claude_id).then_some(s.claude_id.as_str());
                    integration::reopen_cmd(&s.workdir, saved)
                }),
            };
            if let Some(l) = self.inner.lock().unwrap().live.get_mut(id) {
                l.job = job;
            }
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
                job: None,
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
        let mut note_changed = false;
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if !inner.state.sessions.iter().any(|s| s.id == id) {
                return;
            }
            match event {
                "ClaudeStart" => {
                    let _ = std::fs::remove_file(integration::prompt_file(&self.setup.paths, id)); // already read
                    // `claude attach <job>` names the job; any other start clears the last one
                    let job = payload["job"].as_str().filter(|j| !j.is_empty()).map(String::from);
                    if let Some(l) = inner.live.get_mut(id) {
                        l.claude_pid = payload["pid"].as_u64().map(|p| p as u32);
                        l.typed_at = None; // reported in: no longer "on its way"
                        l.job = job.clone();
                        l.by_job = false;
                    }
                    if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                        s.job = job;
                    }
                    self.save(inner);
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
    }
}

fn listed(jobs: &[state::Job]) -> Vec<state::Job> {
    jobs.iter().filter(|j| j.status != Status::Idle).cloned().collect()
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
        note: s.note.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_drops_cursor_escapes() {
        assert_eq!(plain("\x1b[4GYes,\x1b[9GI\x1b[11Gtrust\x1b[17Gthis\r\n"), "Yes, I trust this");
    }
}
