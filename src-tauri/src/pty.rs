use crate::shellenv::Env;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const BLURB: &str = "You are running inside Roost, a session manager. The `roost` CLI is on PATH: `roost new \"<task>\" [--worktree NAME] [--cwd DIR]` starts a sibling Claude session (you become its parent), `roost ls` lists sessions with status, `roost send <name> \"<text>\"` messages a session, `roost stop <name>` stops one. Use them only when the user asks for parallel work or delegation.";

#[derive(Clone, Copy, Debug)]
pub enum Launch<'a> {
    New { prompt: &'a str, worktree: bool },
    Resume,
}

pub fn claude_args(claude_id: &str, name: &str, hooks: &Path, launch: Launch) -> Vec<String> {
    let id_flag = match launch {
        Launch::New { .. } => "--session-id",
        Launch::Resume => "--resume",
    };
    let mut a: Vec<String> = vec![
        id_flag.into(),
        claude_id.into(),
        "-n".into(),
        name.into(),
        "--settings".into(),
        hooks.display().to_string(),
        "--append-system-prompt".into(),
        BLURB.into(),
    ];
    if let Launch::New { prompt, worktree } = launch {
        if worktree {
            a.extend(["-w".into(), name.into()]);
        }
        if !prompt.trim().is_empty() {
            if prompt.starts_with('-') {
                a.push("--".into()); // so the prompt is not parsed as a flag
            }
            a.push(prompt.into());
        }
    }
    a
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

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

impl PtyManager {
    pub fn spawn(
        &self,
        id: &str,
        s: Spawn,
        on_output: impl Fn(String) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> io::Result<()> {
        let size = PtySize { rows: s.rows, cols: s.cols, pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size).map_err(io_err)?;
        let mut cmd = CommandBuilder::new(&s.program);
        cmd.args(&s.args);
        cmd.cwd(&s.cwd);
        cmd.env_clear();
        for (k, v) in &s.env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(io_err)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(io_err)?;
        let writer = pair.master.take_writer().map_err(io_err)?;
        let gen = self.gen.fetch_add(1, Ordering::SeqCst);
        let pid = child.process_id();
        self.procs
            .lock()
            .unwrap()
            .insert(id.to_string(), Proc { gen, master: pair.master, writer, pid });

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
        });
        let procs = self.procs.clone();
        let id = id.to_string();
        thread::spawn(move || {
            let _ = child.wait();
            {
                let mut map = procs.lock().unwrap();
                if map.get(&id).map(|p| p.gen) == Some(gen) {
                    map.remove(&id);
                }
            }
            on_exit();
        });
        Ok(())
    }

    pub fn write(&self, id: &str, data: &[u8]) -> io::Result<()> {
        let mut map = self.procs.lock().unwrap();
        let p = map
            .get_mut(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "session not running"))?;
        p.writer.write_all(data)?;
        p.writer.flush()
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if let Some(p) = self.procs.lock().unwrap().get(id) {
            let _ = p.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.procs.lock().unwrap().contains_key(id)
    }

    /// SIGTERM to the process group; SIGKILL after 3 s if the same process is still there.
    pub fn kill(&self, id: &str) {
        let target = self.procs.lock().unwrap().get(id).and_then(|p| p.pid.map(|pid| (p.gen, pid)));
        let Some((gen, pid)) = target else { return };
        unsafe {
            libc::killpg(pid as i32, libc::SIGTERM);
        }
        let procs = self.procs.clone();
        let id = id.to_string();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(3));
            if procs.lock().unwrap().get(&id).map(|p| p.gen) == Some(gen) {
                unsafe {
                    libc::killpg(pid as i32, libc::SIGKILL);
                }
            }
        });
    }

    /// Used at quit: SIGTERM everything, give it 300 ms, then SIGKILL what's left.
    pub fn kill_all(&self) {
        let pids: Vec<u32> = self.procs.lock().unwrap().values().filter_map(|p| p.pid).collect();
        for pid in &pids {
            unsafe {
                libc::killpg(*pid as i32, libc::SIGTERM);
            }
        }
        thread::sleep(Duration::from_millis(300));
        for pid in &pids {
            unsafe {
                libc::killpg(*pid as i32, libc::SIGKILL);
            }
        }
    }
}

#[derive(Default)]
pub struct Utf8Carry {
    tail: Vec<u8>,
}

pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Env,
    pub cols: u16,
    pub rows: u16,
}

struct Proc {
    gen: u64,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    pid: Option<u32>,
}

#[derive(Clone, Default)]
pub struct PtyManager {
    procs: Arc<Mutex<HashMap<String, Proc>>>,
    gen: Arc<AtomicU64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn new_session_args() {
        let a = claude_args("C1", "fix-auth", Path::new("/h.json"), Launch::New { prompt: "fix auth", worktree: false });
        assert_eq!(
            a,
            s(&["--session-id", "C1", "-n", "fix-auth", "--settings", "/h.json", "--append-system-prompt", BLURB, "fix auth"])
        );
    }

    #[test]
    fn worktree_and_dash_prompt_args() {
        let a = claude_args("C1", "v", Path::new("/h.json"), Launch::New { prompt: "-v flag is broken", worktree: true });
        assert_eq!(&a[8..], &s(&["-w", "v", "--", "-v flag is broken"])[..]);
    }

    #[test]
    fn empty_prompt_and_resume_args() {
        let a = claude_args("C1", "n", Path::new("/h.json"), Launch::New { prompt: "  ", worktree: false });
        assert_eq!(a.last().unwrap(), BLURB);
        let r = claude_args("C1", "n", Path::new("/h.json"), Launch::Resume);
        assert_eq!(r, s(&["--resume", "C1", "-n", "n", "--settings", "/h.json", "--append-system-prompt", BLURB]));
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
        let m = PtyManager::default();
        let (out, exit) = spawn_cmd(&m, "a", &["-c", "printf 'hi ⏺'"]);
        assert!(collect_until(&out, "hi ⏺").contains("hi ⏺"));
        exit.recv_timeout(Duration::from_secs(5)).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(!m.is_running("a"));
    }

    #[test]
    fn writes_input_and_kills() {
        let m = PtyManager::default();
        let (out, exit) = spawn_cmd(&m, "b", &["-c", "cat"]);
        assert!(m.is_running("b"));
        m.write("b", b"ping\n").unwrap();
        assert!(collect_until(&out, "ping").contains("ping"));
        m.resize("b", 100, 30);
        m.kill("b");
        exit.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(m.write("b", b"x").is_err() || !m.is_running("b"));
    }
}
