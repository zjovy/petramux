//! The app's side of the terminals: a client of the helper (`host.rs`) that owns them, so
//! sessions outlive the app. Same API as when the app owned them itself, plus `attach`.
use crate::host::{HostRequest, HostResponse, Term};
use crate::shellenv::Env;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub const BLURB: &str = "You are running inside petramux, a session manager. The `petramux` CLI is on PATH: `petramux new \"<task>\" [--worktree NAME] [--cwd DIR]` starts a sibling Claude session (you become its parent), `petramux ls` lists sessions with status, `petramux send <name> \"<text>\"` messages a session, `petramux stop <name>` stops one. Use them only when the user asks for parallel work or delegation.";

#[derive(Default)]
pub struct Utf8Carry {
    tail: Vec<u8>,
}

impl Utf8Carry {
    /// Decodes what it can; holds back an incomplete trailing sequence for the next read.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.tail.extend_from_slice(bytes);
        let cut = incomplete_tail_start(&self.tail);
        let rest = self.tail.split_off(cut);
        let text = String::from_utf8_lossy(&self.tail).into_owned();
        self.tail = rest;
        text
    }
}

/// Index where a trailing, not-yet-complete UTF-8 sequence starts (or len if none).
fn incomplete_tail_start(buf: &[u8]) -> usize {
    for i in (buf.len().saturating_sub(3)..buf.len()).rev() {
        let b = buf[i];
        if b & 0xC0 == 0x80 {
            continue; // continuation byte, keep looking for the lead byte
        }
        let need = match b {
            0xF0..=0xFF => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        return if buf.len() - i < need { i } else { buf.len() };
    }
    buf.len()
}

pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Env,
    pub cols: u16,
    pub rows: u16,
}

struct Ctl {
    read: BufReader<UnixStream>,
    write: UnixStream,
}

#[derive(Clone)]
pub struct PtyManager {
    sock: PathBuf,
    /// Starts the helper when none answers (`None`: whoever made this runs it).
    exe: Option<PathBuf>,
    ctl: Arc<Mutex<Option<Ctl>>>,
    streams: Arc<Mutex<Vec<UnixStream>>>,
}

fn other(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

impl PtyManager {
    pub fn new(sock: PathBuf, exe: Option<PathBuf>) -> Self {
        Self { sock, exe, ctl: Arc::default(), streams: Arc::default() }
    }

    /// A connection to the helper, starting it first if nothing answers.
    fn connect(&self) -> io::Result<UnixStream> {
        if let Ok(c) = UnixStream::connect(&self.sock) {
            return Ok(c);
        }
        let exe = self.exe.as_ref().ok_or_else(|| other("the terminal helper isn't running"))?;
        let mut cmd = Command::new(exe);
        cmd.arg("pty-host").arg(&self.sock).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        // Its own session: it must outlive the app (it's still the app's, for macOS permissions).
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        thread::spawn(move || {
            let _ = child.wait(); // reap it if it exits while the app is still here
        });
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match UnixStream::connect(&self.sock) {
                Ok(c) => return Ok(c),
                Err(e) if Instant::now() > end => return Err(e),
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    fn call(&self, req: &HostRequest) -> io::Result<HostResponse> {
        let mut ctl = self.ctl.lock().unwrap();
        for attempt in 0..2 {
            if ctl.is_none() {
                let conn = self.connect()?;
                conn.set_read_timeout(Some(Duration::from_secs(5)))?;
                *ctl = Some(Ctl { read: BufReader::new(conn.try_clone()?), write: conn });
            }
            let c = ctl.as_mut().unwrap();
            let sent = (|| {
                let mut line = serde_json::to_vec(req)?;
                line.push(b'\n');
                c.write.write_all(&line)?;
                let mut resp = String::new();
                if c.read.read_line(&mut resp)? == 0 {
                    return Err(other("the terminal helper hung up"));
                }
                Ok::<_, io::Error>(serde_json::from_str::<HostResponse>(&resp)?)
            })();
            match sent {
                Ok(r) => return Ok(r),
                Err(e) if attempt == 1 => return Err(e),
                Err(_) => *ctl = None, // the helper restarted: reconnect once
            }
        }
        unreachable!()
    }

    pub fn spawn(
        &self,
        id: &str,
        s: Spawn,
        on_output: impl Fn(String) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> io::Result<()> {
        let env = s.env.into_iter().collect();
        let req = HostRequest::Spawn { id: id.into(), program: s.program, args: s.args, cwd: s.cwd, env, cols: s.cols, rows: s.rows };
        let r = self.call(&req)?;
        if !r.ok {
            let msg = r.error.unwrap_or_default();
            let kind = if msg.contains("already running") { io::ErrorKind::AlreadyExists } else { io::ErrorKind::Other };
            return Err(io::Error::new(kind, msg));
        }
        self.attach(id, on_output, on_exit)
    }

    /// Streams a running terminal's output: what it printed before, then what it prints now.
    /// `on_exit` runs when the terminal ends (not when this client detaches).
    pub fn attach(&self, id: &str, on_output: impl Fn(String) + Send + 'static, on_exit: impl FnOnce() + Send + 'static) -> io::Result<()> {
        let mut conn = self.connect()?;
        let mut line = serde_json::to_vec(&HostRequest::Attach { id: id.into() })?;
        line.push(b'\n');
        conn.write_all(&line)?;
        let mut reader = BufReader::new(conn.try_clone()?);
        let mut resp = String::new();
        reader.read_line(&mut resp)?;
        let r: HostResponse = serde_json::from_str(&resp)?;
        if !r.ok {
            return Err(io::Error::new(io::ErrorKind::NotFound, r.error.unwrap_or_default()));
        }
        self.streams.lock().unwrap().push(conn);
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        // Coalesce output into at most one message per 16 ms.
        thread::spawn(move || {
            let mut carry = Utf8Carry::default();
            while let Ok(mut batch) = rx.recv() {
                let deadline = Instant::now() + Duration::from_millis(16);
                while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                    match rx.recv_timeout(left) {
                        Ok(more) => batch.extend(more),
                        Err(_) => break,
                    }
                }
                let text = carry.push(&batch);
                if !text.is_empty() {
                    on_output(text);
                }
            }
            on_exit();
        });
        Ok(())
    }

    pub fn write(&self, id: &str, data: &[u8]) -> io::Result<()> {
        let r = self.call(&HostRequest::Write { id: id.into(), data: String::from_utf8_lossy(data).into_owned() })?;
        if r.ok {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, r.error.unwrap_or_default()))
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        let _ = self.call(&HostRequest::Resize { id: id.into(), cols, rows });
    }

    /// The terminals the helper holds.
    pub fn running(&self) -> Vec<Term> {
        self.call(&HostRequest::List).ok().and_then(|r| r.terms).unwrap_or_default()
    }

    /// Leader of the terminal's foreground process group: whoever has the keyboard.
    pub fn foreground(&self, id: &str) -> Option<i32> {
        self.running().into_iter().find(|t| t.id == id).and_then(|t| t.fg)
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.running().iter().any(|t| t.id == id)
    }

    /// SIGHUP to the process group, SIGKILL after 3 s (in the helper).
    pub fn kill(&self, id: &str) {
        let _ = self.call(&HostRequest::Kill { id: id.into() });
    }

    /// Stops listening; the terminals keep running in the helper.
    pub fn detach(&self) {
        for s in self.streams.lock().unwrap().drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        *self.ctl.lock().unwrap() = None;
    }

    /// Ends every terminal and the helper.
    pub fn stop_all(&self) {
        let _ = self.call(&HostRequest::Quit);
        *self.ctl.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn utf8_split_across_reads_is_reassembled() {
        let text = "a⏺b🚀c";
        let bytes = text.as_bytes();
        for cut in 0..=bytes.len() {
            let mut c = Utf8Carry::default();
            let out = c.push(&bytes[..cut]) + &c.push(&bytes[cut..]);
            assert_eq!(out, text, "cut at {cut}");
        }
    }

    #[test]
    fn invalid_bytes_become_replacement_chars() {
        let mut c = Utf8Carry::default();
        assert_eq!(c.push(b"a\xFFb"), "a\u{FFFD}b");
    }

    fn env() -> Env {
        let mut e = Env::new();
        e.insert("PATH".into(), "/bin:/usr/bin".into());
        e
    }

    /// A helper running in this test process (it never exits on its own).
    fn manager() -> (PtyManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("pty.sock");
        let s2 = sock.clone();
        thread::spawn(move || crate::host::serve(&s2, false));
        let end = Instant::now() + Duration::from_secs(3);
        while UnixStream::connect(&sock).is_err() && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        (PtyManager::new(sock, None), dir)
    }

    fn spawn_cmd(m: &PtyManager, id: &str, args: &[&str]) -> (mpsc::Receiver<String>, mpsc::Receiver<()>) {
        let (otx, orx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        m.spawn(
            id,
            Spawn { program: "/bin/sh".into(), args: s(args), cwd: "/".into(), env: env(), cols: 80, rows: 24 },
            move |t| {
                let _ = otx.send(t);
            },
            move || {
                let _ = etx.send(());
            },
        )
        .unwrap();
        (orx, erx)
    }

    fn collect_until(rx: &mpsc::Receiver<String>, needle: &str) -> String {
        let mut all = String::new();
        let end = Instant::now() + Duration::from_secs(5);
        while !all.contains(needle) && Instant::now() < end {
            if let Ok(t) = rx.recv_timeout(Duration::from_millis(100)) {
                all += &t;
            }
        }
        all
    }

    #[test]
    fn spawns_streams_output_and_reports_exit() {
        let (m, _d) = manager();
        let (out, exit) = spawn_cmd(&m, "a", &["-c", "printf 'hi ⏺'"]);
        assert!(collect_until(&out, "hi ⏺").contains("hi ⏺"));
        exit.recv_timeout(Duration::from_secs(5)).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(!m.is_running("a"));
    }

    #[test]
    fn writes_input_and_kills() {
        let (m, _d) = manager();
        let (out, exit) = spawn_cmd(&m, "b", &["-c", "cat"]);
        assert!(m.is_running("b"));
        m.write("b", b"ping\n").unwrap();
        assert!(collect_until(&out, "ping").contains("ping"));
        m.resize("b", 100, 30);
        m.kill("b");
        exit.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(m.write("b", b"x").is_err() || !m.is_running("b"));
    }

    #[test]
    fn spawn_refuses_an_id_that_is_already_running() {
        let (m, _d) = manager();
        let (_out, exit) = spawn_cmd(&m, "x", &["-c", "cat"]);
        let again = m.spawn(
            "x",
            Spawn { program: "/bin/sh".into(), args: s(&["-c", "cat"]), cwd: "/".into(), env: env(), cols: 80, rows: 24 },
            |_| {},
            || {},
        );
        assert_eq!(again.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        m.kill("x");
        exit.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn a_session_that_stops_reading_does_not_block_the_others() {
        let (m, _d) = manager();
        let (_o1, _e1) = spawn_cmd(&m, "stuck", &["-c", "stty raw -echo; sleep 5"]);
        let (out, _e2) = spawn_cmd(&m, "ok", &["-c", "cat"]);
        thread::sleep(Duration::from_millis(300));
        let m2 = m.clone();
        thread::spawn(move || {
            let _ = m2.write("stuck", &vec![b'x'; 256 * 1024]);
        });
        thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        m.resize("ok", 100, 30);
        assert!(m.is_running("ok"));
        m.write("ok", b"ping\n").unwrap();
        assert!(start.elapsed() < Duration::from_secs(1), "blocked for {:?}", start.elapsed());
        assert!(collect_until(&out, "ping").contains("ping"));
        m.kill("stuck");
        m.kill("ok");
    }

    #[test]
    fn kill_ends_an_interactive_shell_promptly() {
        if !std::path::Path::new("/bin/zsh").exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (m, _d) = manager();
        let mut e = env();
        e.insert("HOME".into(), dir.path().display().to_string());
        e.insert("ZDOTDIR".into(), dir.path().display().to_string());
        let (etx, erx) = mpsc::channel();
        let spec = Spawn { program: "/bin/zsh".into(), args: s(&["-i"]), cwd: "/".into(), env: e, cols: 80, rows: 24 };
        m.spawn("z", spec, |_| {}, move || {
            let _ = etx.send(());
        })
        .unwrap();
        thread::sleep(Duration::from_millis(500));
        let start = Instant::now();
        m.kill("z");
        erx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(start.elapsed() < Duration::from_millis(1500), "took {:?}", start.elapsed());
    }

    #[test]
    fn a_client_that_comes_back_gets_what_it_missed_then_live_output() {
        let (m, _d) = manager();
        let (out, _exit) = spawn_cmd(&m, "c", &["-c", "echo before; cat"]);
        assert!(collect_until(&out, "before").contains("before"));
        m.detach(); // the app quits; the terminal keeps running

        let again = PtyManager::new(m.sock.clone(), None);
        assert!(again.running().iter().any(|t| t.id == "c"));
        let (tx, rx) = mpsc::channel();
        let (etx, erx) = mpsc::channel();
        again
            .attach("c", move |t| {
                let _ = tx.send(t);
            }, move || {
                let _ = etx.send(());
            })
            .unwrap();
        again.write("c", b"after\n").unwrap();
        let seen = collect_until(&rx, "after");
        assert_eq!(seen.matches("before").count(), 1, "{seen:?}");
        assert!(seen.contains("after"), "{seen:?}");
        assert!(erx.try_recv().is_err(), "detaching the first client must not look like an exit");
        again.kill("c");
        erx.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn attaching_to_a_terminal_that_is_not_there_says_so() {
        let (m, _d) = manager();
        let err = m.attach("nope", |_| {}, || {}).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
