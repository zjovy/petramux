use crate::paths::sock_from_env;
use crate::sock::{call, LsRow, Request, Response};
use crate::state::Status;
use std::{
    io::{self, Read},
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub const USAGE: &str = "usage:
  petramux new \"<prompt>\" [--worktree NAME] [--cwd DIR] [--name NAME]
  petramux ls [--json]
  petramux send <name|id> \"<text>\"
  petramux stop <name|id>
  petramux say \"<text>\"            post to your workspace's chat (@name or @all to deliver it)
  petramux chat [--tail N] [--wait] your workspace's chat: the last N messages (20) and all you haven't read;
                                    --wait first waits until a message @mentions you";

#[derive(Debug, PartialEq)]
pub enum Cmd {
    New { prompt: String, worktree: Option<String>, cwd: Option<String>, name: Option<String> },
    Ls { json: bool },
    Send { target: String, text: String },
    Stop { target: String },
    Say { text: String },
    Chat { tail: usize, wait: bool },
    Hook { event: String },
    /// The terminal helper the app starts; it runs until it has nothing to hold.
    PtyHost { sock: String },
    Help,
}

pub fn is_cli(arg: &str) -> bool {
    matches!(arg, "new" | "ls" | "send" | "stop" | "say" | "chat" | "hook" | "pty-host" | "help" | "--help" | "-h")
}

pub fn parse(args: &[String]) -> Result<Cmd, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    match cmd {
        "new" => {
            let (mut prompt, mut worktree, mut cwd, mut name) = (None, None, None, None);
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--worktree" => worktree = Some(value(&rest, &mut i)?),
                    "--cwd" => cwd = Some(value(&rest, &mut i)?),
                    "--name" => name = Some(value(&rest, &mut i)?),
                    p if prompt.is_none() => prompt = Some(p.to_string()),
                    extra => return Err(format!("unexpected argument: {extra}")),
                }
                i += 1;
            }
            Ok(Cmd::New { prompt: prompt.unwrap_or_default(), worktree, cwd, name })
        }
        "ls" => Ok(Cmd::Ls { json: rest.contains(&"--json") }),
        "send" => match rest.as_slice() {
            [target, text] => Ok(Cmd::Send { target: target.to_string(), text: text.to_string() }),
            _ => Err("send takes <name|id> and \"<text>\"".into()),
        },
        "stop" => match rest.as_slice() {
            [target] => Ok(Cmd::Stop { target: target.to_string() }),
            _ => Err("stop takes <name|id>".into()),
        },
        "say" => match rest.as_slice() {
            [text] => Ok(Cmd::Say { text: text.to_string() }),
            _ => Err("say takes \"<text>\" (quote it)".into()),
        },
        "chat" => {
            let (mut tail, mut wait) = (20, false);
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match *a {
                    "--wait" => wait = true,
                    "--tail" => tail = it.next().and_then(|n| n.parse().ok()).ok_or("--tail needs a number")?,
                    _ => return Err("chat takes [--tail N] [--wait]".into()),
                }
            }
            Ok(Cmd::Chat { tail, wait })
        }
        "hook" => match rest.as_slice() {
            [event] => Ok(Cmd::Hook { event: event.to_string() }),
            _ => Err("hook takes <Event>".into()),
        },
        "pty-host" => match rest.as_slice() {
            [sock] => Ok(Cmd::PtyHost { sock: sock.to_string() }),
            _ => Err("pty-host takes <socket>".into()),
        },
        "help" | "--help" | "-h" => Ok(Cmd::Help),
        other => Err(format!("unknown command: {other}")),
    }
}

fn value(rest: &[&str], i: &mut usize) -> Result<String, String> {
    let flag = rest[*i];
    *i += 1;
    rest.get(*i).map(|s| s.to_string()).ok_or_else(|| format!("{flag} needs a value"))
}

/// Never fails, and prints only what petramux hands back for Claude (chat messages that arrived
/// mid-turn): a hook must not block or break Claude.
pub fn run_hook(sock: &Path, event: &str, stdin: impl Read, session: Option<String>) {
    let Some(session) = session else { return };
    let mut input = String::new();
    let _ = stdin.take(1 << 20).read_to_string(&mut input);
    let mut payload = serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);
    // Claude waits for this hook, so the snapshot is taken before it edits anything.
    if event == "UserPromptSubmit" {
        if let Some(base) = payload["cwd"].as_str().and_then(|d| crate::git::snapshot(Path::new(d))) {
            payload["roost_base"] = base.into();
        }
    }
    let req = Request::Hook { session, event: event.into(), payload };
    if let Some(reply) = call(sock, &req, Duration::from_millis(500)).ok().and_then(|r| r.hook) {
        println!("{reply}");
    }
}

pub fn run(args: &[String]) -> i32 {
    let cmd = match parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("petramux: {e}\n\n{USAGE}");
            return 2;
        }
    };
    let sock = sock_from_env();
    let me = std::env::var("ROOST_SESSION").ok();
    match cmd {
        Cmd::Hook { event } => {
            run_hook(&sock, &event, io::stdin(), me);
            0
        }
        Cmd::Help => {
            println!("{USAGE}");
            0
        }
        Cmd::PtyHost { sock } => match crate::host::serve(Path::new(&sock), true) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("petramux pty-host: {e}");
                1
            }
        },
        Cmd::New { prompt, worktree, cwd, name } => {
            let cwd = cwd.unwrap_or_else(|| ".".into());
            let cwd = std::path::absolute(&cwd).map(|p| p.display().to_string()).unwrap_or(cwd);
            let req = Request::New { prompt, cwd, worktree, name, parent: me };
            finish(call_launching(&sock, &req), |r| println!("{}", r.name.unwrap_or_default()))
        }
        Cmd::Ls { json } => finish(call(&sock, &Request::Ls, Duration::from_secs(2)), |r| {
            print_ls(&r.sessions.unwrap_or_default(), json)
        }),
        Cmd::Send { target, text } => {
            let req = Request::Send { target, text, from: me };
            finish(call(&sock, &req, Duration::from_secs(20)), |_| {})
        }
        Cmd::Stop { target } => finish(call(&sock, &Request::Stop { target }, Duration::from_secs(2)), |_| {}),
        Cmd::Say { text } => finish(call(&sock, &Request::Say { from: me, text }, Duration::from_secs(5)), |_| {}),
        Cmd::Chat { tail, wait } => {
            let req = Request::Chat { from: me, tail, wait, pid: Some(std::process::id()) };
            // waiting has no end of its own: it ends when someone names this session
            let t = if wait { Duration::from_secs(7 * 24 * 3600) } else { Duration::from_secs(2) };
            finish(call(&sock, &req, t), |r| {
                let msgs = r.messages.unwrap_or_default();
                let new = r.unread.unwrap_or(0);
                println!("#{}", crate::chat::chat_tag(&r.name.unwrap_or_default()));
                for (i, m) in msgs.iter().enumerate() {
                    if new > 0 && i == msgs.len() - new {
                        println!("── {new} new ──");
                    }
                    println!("{} {}: {}", clock(m.ts), m.from, m.text);
                }
                if wait {
                    println!("\n(You were mentioned. Reply with `petramux say`; start `petramux chat --wait` in the background again before you stop.)");
                }
            })
        }
    }
}

/// `HH:MM` in local time.
fn clock(ms: u64) -> String {
    let secs = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

fn finish(res: io::Result<Response>, ok: impl FnOnce(Response)) -> i32 {
    match res {
        Err(_) => {
            eprintln!("petramux isn't running");
            1
        }
        Ok(r) if !r.ok => {
            eprintln!("petramux: {}", r.error.unwrap_or_default());
            1
        }
        Ok(r) => {
            ok(r);
            0
        }
    }
}

/// `new` starts the app if needed. Only retries when nothing is listening,
/// so a slow but delivered request is never sent twice.
fn call_launching(sock: &Path, req: &Request) -> io::Result<Response> {
    let t = Duration::from_secs(10);
    let not_listening = |e: &io::Error| matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused);
    match call(sock, req, t) {
        Err(e) if not_listening(&e) => {}
        other => return other,
    }
    let _ = std::process::Command::new("open").args(["-b", "dev.roost.app"]).status();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        thread::sleep(Duration::from_millis(200));
        match call(sock, req, t) {
            Err(e) if not_listening(&e) && Instant::now() < end => continue,
            other => return other,
        }
    }
}

fn print_ls(rows: &[LsRow], json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(rows).unwrap_or_default());
        return;
    }
    for r in rows {
        let parent = r
            .parent
            .as_ref()
            .and_then(|p| rows.iter().find(|x| &x.id == p))
            .map(|p| format!("  ↳ {}", p.name))
            .unwrap_or_default();
        println!("{:<26} {:<10} {}{}", r.name, status_word(r.status), r.repo, parent);
    }
}

fn status_word(s: Status) -> &'static str {
    match s {
        Status::Starting => "starting",
        Status::Shell => "shell",
        Status::Asleep => "asleep",
        Status::Working => "working",
        Status::NeedsYou => "needs-you",
        Status::Idle => "idle",
        Status::Exited => "exited",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc, Mutex};

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_new_with_flags() {
        assert_eq!(
            parse(&a(&["new", "fix it", "--worktree", "wt", "--cwd", "/r", "--name", "n"])).unwrap(),
            Cmd::New { prompt: "fix it".into(), worktree: Some("wt".into()), cwd: Some("/r".into()), name: Some("n".into()) }
        );
        assert_eq!(
            parse(&a(&["new"])).unwrap(),
            Cmd::New { prompt: "".into(), worktree: None, cwd: None, name: None }
        );
    }

    #[test]
    fn dash_prompt_is_a_prompt_not_a_flag() {
        let Cmd::New { prompt, .. } = parse(&a(&["new", "-v flag is broken"])).unwrap() else { panic!() };
        assert_eq!(prompt, "-v flag is broken");
    }

    #[test]
    fn parses_other_commands() {
        assert_eq!(parse(&a(&["ls", "--json"])).unwrap(), Cmd::Ls { json: true });
        assert_eq!(parse(&a(&["send", "x", "hi"])).unwrap(), Cmd::Send { target: "x".into(), text: "hi".into() });
        assert_eq!(parse(&a(&["stop", "x"])).unwrap(), Cmd::Stop { target: "x".into() });
        assert_eq!(parse(&a(&["say", "@all a\nb"])).unwrap(), Cmd::Say { text: "@all a\nb".into() });
        assert_eq!(parse(&a(&["chat"])).unwrap(), Cmd::Chat { tail: 20, wait: false });
        assert_eq!(parse(&a(&["chat", "--tail", "5"])).unwrap(), Cmd::Chat { tail: 5, wait: false });
        assert_eq!(parse(&a(&["chat", "--wait", "--tail", "5"])).unwrap(), Cmd::Chat { tail: 5, wait: true });
        assert!(parse(&a(&["say"])).is_err());
        assert!(parse(&a(&["say", "a", "b"])).is_err());
        assert!(parse(&a(&["chat", "--tail", "x"])).is_err());
        assert_eq!(parse(&a(&["hook", "Stop"])).unwrap(), Cmd::Hook { event: "Stop".into() });
        assert_eq!(parse(&a(&["--help"])).unwrap(), Cmd::Help);
    }

    #[test]
    fn rejects_bad_usage() {
        assert!(parse(&a(&["new", "--cwd"])).is_err());
        assert!(parse(&a(&["new", "a", "b"])).is_err());
        assert!(parse(&a(&["send", "x"])).is_err());
        assert!(parse(&a(&["bogus"])).is_err());
    }

    #[test]
    fn hook_without_app_returns_fast() {
        let start = Instant::now();
        run_hook(Path::new("/tmp/roost-nope.sock"), "Stop", &b"{}"[..], Some("s".into()));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn hook_forwards_session_event_and_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.sock");
        let (tx, rx) = mpsc::channel();
        let tx = Arc::new(Mutex::new(tx));
        crate::sock::serve(&path, Arc::new(move |req| {
            tx.lock().unwrap().send(req).unwrap();
            Response::ok()
        }), None)
        .unwrap();
        run_hook(&path, "SessionStart", &br#"{"cwd":"/r"}"#[..], Some("s1".into()));
        let got = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            got,
            Request::Hook { session: "s1".into(), event: "SessionStart".into(), payload: serde_json::json!({"cwd": "/r"}) }
        );
        run_hook(&path, "Stop", &b"{}"[..], None);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "no session id, no call");
    }
}
