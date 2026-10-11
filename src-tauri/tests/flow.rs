use roost_lib::engine::{Attention, Core, Sink, View};
use roost_lib::paths::Paths;
use roost_lib::shellenv::Env;
use roost_lib::sock::{self, Request};
use roost_lib::state::Status;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Rec {
    snapshots: Mutex<Vec<Vec<View>>>,
    changes: Mutex<Vec<(String, Status)>>,
    output: Mutex<String>,
    attention: Mutex<Vec<&'static str>>,
}

impl Sink for Rec {
    fn sessions_changed(&self, views: Vec<View>) {
        self.snapshots.lock().unwrap().push(views);
    }
    fn session_changed(&self, v: View) {
        self.changes.lock().unwrap().push((v.id, v.status));
    }
    fn output(&self, _id: &str, text: String) {
        self.output.lock().unwrap().push_str(&text);
    }
    fn attention(&self, _v: &View, kind: Attention) {
        self.attention.lock().unwrap().push(match kind {
            Attention::NeedsYou => "needs_you",
            Attention::Done => "done",
        });
    }
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn setup(dir: &std::path::Path) -> (Arc<Core>, Arc<Rec>, Paths) {
    let paths = Paths::new(dir.join("support"));
    let mut env = Env::new();
    env.insert("PATH".into(), "/bin:/usr/bin".into());
    env.insert("HOME".into(), dir.display().to_string());
    env.insert("ROOST_BIN".into(), env!("CARGO_BIN_EXE_roost").into());
    let fake = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake-claude.sh"));
    let rec = Arc::new(Rec::default());
    let core = Core::new(paths.clone(), env, Some(fake), rec.clone());
    sock::serve(&paths.sock(), core.handler()).unwrap();
    (core, rec, paths)
}

fn statuses(rec: &Rec, id: &str) -> Vec<Status> {
    rec.changes.lock().unwrap().iter().filter(|(i, _)| i == id).map(|(_, s)| *s).collect()
}

#[test]
fn new_session_runs_through_every_status_then_send_and_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().canonicalize().unwrap().display().to_string();
    let (core, rec, paths) = setup(tmp.path());
    let t = Duration::from_secs(5);

    // A hook for a session that doesn't exist is ignored, not an error.
    let stray = Request::Hook { session: "nope".into(), event: "Stop".into(), payload: serde_json::json!({}) };
    assert!(sock::call(&paths.sock(), &stray, t).unwrap().ok);
    assert!(core.views().is_empty());

    let new = Request::New { prompt: "fix it".into(), cwd: repo.clone(), worktree: None, name: None, parent: None };
    let resp = sock::call(&paths.sock(), &new, t).unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.name.as_deref(), Some("fix-it"));
    let id = resp.id.unwrap();
    assert_eq!(rec.snapshots.lock().unwrap()[0][0].status, Status::Starting);

    wait("five status changes", || statuses(&rec, &id).len() >= 5);
    assert_eq!(
        statuses(&rec, &id),
        vec![Status::Idle, Status::Working, Status::NeedsYou, Status::Working, Status::Idle]
    );
    assert_eq!(*rec.attention.lock().unwrap(), vec!["needs_you", "done"]);
    assert_eq!(core.views()[0].workdir, repo);
    assert!(std::fs::read_to_string(paths.state()).unwrap().contains("fake-claude-id"));

    let out = rec.output.lock().unwrap().clone();
    assert!(out.contains("--session-id") && out.contains("--settings") && out.contains("fix it"));

    let send = Request::Send { target: "fix-it".into(), text: "hello".into(), from: Some(id.clone()) };
    assert!(sock::call(&paths.sock(), &send, t).unwrap().ok);
    wait("sent text echoed", || rec.output.lock().unwrap().contains("[from fix-it] hello"));
    assert!(rec.output.lock().unwrap().contains("200~"), "bracketed paste markers");

    let ls = sock::call(&paths.sock(), &Request::Ls, t).unwrap().sessions.unwrap();
    assert_eq!(ls[0].name, "fix-it");

    assert!(sock::call(&paths.sock(), &Request::Stop { target: "fix-it".into() }, t).unwrap().ok);
    wait("exited", || core.views()[0].status == Status::Exited);
}

#[test]
fn quit_and_relaunch_brings_sessions_back_asleep_then_resumes_on_select() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().canonicalize().unwrap().display().to_string();
    let (core, rec, _paths) = setup(tmp.path());
    let v = core
        .new_session(roost_lib::engine::NewSession {
            prompt: "a".into(),
            repo: repo.clone(),
            worktree: false,
            name: None,
            parent: None,
        })
        .unwrap();
    wait("first SessionStart", || !statuses(&rec, &v.id).is_empty());
    core.shutdown();
    drop(core);

    let (core2, rec2, _) = setup(tmp.path());
    assert_eq!(core2.views()[0].status, Status::Asleep);
    core2.select(&v.id).unwrap();
    wait("resumed output", || rec2.output.lock().unwrap().contains("--resume fake-claude-id"));
    core2.shutdown();
}

#[test]
fn rows_get_unique_names_and_children_record_their_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().canonicalize().unwrap().display().to_string();
    let (core, _rec, _) = setup(tmp.path());
    let mk = |parent: Option<String>| roost_lib::engine::NewSession {
        prompt: "same".into(),
        repo: repo.clone(),
        worktree: false,
        name: None,
        parent,
    };
    let a = core.new_session(mk(None)).unwrap();
    let b = core.new_session(mk(Some(a.id.clone()))).unwrap();
    assert_eq!((a.name.as_str(), b.name.as_str()), ("same", "same-2"));
    assert_eq!(b.parent.as_deref(), Some(a.id.as_str()));
    let renamed = core.rename(&b.id, "Same").unwrap();
    assert_eq!(renamed.name, "same-2", "renaming onto a taken name keeps it unique");
    core.close(&a.id);
    assert_eq!(core.views().len(), 1);
    core.shutdown();
}
