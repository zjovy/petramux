//! The terminal helper (`petramux pty-host <socket>`): owns every session's PTY so sessions
//! keep running when the app quits or updates. The app is a client that reconnects.
//! See docs/specs/2026-10-13-petramux-keep-sessions-running.md.
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const PROTOCOL: u32 = 1;
/// Output kept per terminal for a client that attaches later.
const RING: usize = 1 << 20;
const IDLE_EXIT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HostRequest {
    Hello,
    Spawn { id: String, program: PathBuf, args: Vec<String>, cwd: PathBuf, env: BTreeMap<String, String>, cols: u16, rows: u16 },
    Write { id: String, data: String },
    Resize { id: String, cols: u16, rows: u16 },
    Kill { id: String },
    List,
    /// Turns the connection into the terminal's output stream: replay, then live, then EOF.
    Attach { id: String },
    Quit,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms: Option<Vec<Term>>,
}

/// A running terminal: its shell's pid and the leader of its foreground process group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Term {
    pub id: String,
    pub pid: Option<u32>,
    pub fg: Option<i32>,
}

impl HostResponse {
    fn ok() -> Self {
        Self { ok: true, ..Default::default() }
    }
    fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()), ..Default::default() }
    }
}

#[derive(Default)]
struct Out {
    ring: VecDeque<u8>,
    cut: bool,
    subs: Vec<UnixStream>,
}

impl Out {
    fn push(&mut self, bytes: &[u8]) {
        self.ring.extend(bytes);
        if self.ring.len() > RING {
            let extra = self.ring.len() - RING;
            self.ring.drain(..extra);
            self.cut = true;
        }
        self.subs.retain_mut(|s| s.write_all(bytes).is_ok());
    }

    /// The buffer as a late client should see it: if the start was cut, from the first line break.
    fn replay(&self) -> Vec<u8> {
        let (a, b) = self.ring.as_slices();
        let all = [a, b].concat();
        if !self.cut {
            return all;
        }
        match all.iter().position(|&c| c == b'\n') {
            Some(i) => all[i + 1..].to_vec(),
            None => Vec::new(),
        }
    }
}

struct Proc {
    gen: u64,
    master: Box<dyn MasterPty + Send>,
    // Its own queue and thread: a child that stops reading stalls only its own input.
    input: std::sync::mpsc::Sender<Vec<u8>>,
    pid: Option<u32>,
    out: Arc<Mutex<Out>>,
}

#[derive(Default)]
struct Host {
    procs: Mutex<HashMap<String, Proc>>,
    gen: AtomicU64,
    clients: AtomicUsize,
}

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

impl Host {
    fn spawn(self: &Arc<Self>, id: String, program: PathBuf, args: Vec<String>, cwd: PathBuf, env: BTreeMap<String, String>, cols: u16, rows: u16) -> io::Result<()> {
        let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size).map_err(io_err)?;
        let mut cmd = CommandBuilder::new(&program);
        cmd.args(&args);
        cmd.cwd(&cwd);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(io_err)?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(io_err)?;
        let mut writer = pair.master.take_writer().map_err(io_err)?;
        let gen = self.gen.fetch_add(1, Ordering::SeqCst);
        let pid = child.process_id();
        let out = Arc::new(Mutex::new(Out::default()));
        {
            // Check and insert under one lock: two spawns for one id must not both win.
            let mut map = self.procs.lock().unwrap();
            if map.contains_key(&id) {
                drop(map);
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "session already running"));
            }
            let (input, queue) = std::sync::mpsc::channel::<Vec<u8>>();
            thread::spawn(move || {
                for data in queue {
                    if writer.write_all(&data).and_then(|_| writer.flush()).is_err() {
                        break;
                    }
                }
            });
            map.insert(id.clone(), Proc { gen, master: pair.master, input, pid, out: out.clone() });
        }
        let reader_out = out.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => reader_out.lock().unwrap().push(&buf[..n]),
                }
            }
        });
        let host = self.clone();
        thread::spawn(move || {
            let _ = child.wait();
            {
                let mut map = host.procs.lock().unwrap();
                if map.get(&id).map(|p| p.gen) == Some(gen) {
                    map.remove(&id);
                }
            }
            thread::sleep(Duration::from_millis(100)); // let the reader pass on the last output
            out.lock().unwrap().subs.clear(); // EOF: the terminal is gone
        });
        Ok(())
    }

    /// SIGHUP to the process group (what a closing terminal sends); SIGKILL after 3 s.
    fn kill(self: &Arc<Self>, id: &str) {
        let target = self.procs.lock().unwrap().get(id).and_then(|p| p.pid.map(|pid| (p.gen, pid)));
        let Some((gen, pid)) = target else { return };
        unsafe {
            libc::killpg(pid as i32, libc::SIGHUP);
        }
        let host = self.clone();
        let id = id.to_string();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(3));
            if host.procs.lock().unwrap().get(&id).map(|p| p.gen) == Some(gen) {
                unsafe {
                    libc::killpg(pid as i32, libc::SIGKILL);
                }
            }
        });
    }

    fn kill_all(&self) {
        let pids: Vec<u32> = self.procs.lock().unwrap().values().filter_map(|p| p.pid).collect();
        for pid in &pids {
            unsafe {
                libc::killpg(*pid as i32, libc::SIGHUP);
            }
        }
        thread::sleep(Duration::from_millis(300));
        for pid in &pids {
            unsafe {
                libc::killpg(*pid as i32, libc::SIGKILL);
            }
        }
    }

    fn handle(self: &Arc<Self>, req: HostRequest) -> HostResponse {
        match req {
            HostRequest::Hello => HostResponse { protocol: Some(PROTOCOL), ..HostResponse::ok() },
            HostRequest::Spawn { id, program, args, cwd, env, cols, rows } => match self.spawn(id, program, args, cwd, env, cols, rows) {
                Ok(()) => HostResponse::ok(),
                Err(e) => HostResponse::err(e.to_string()),
            },
            HostRequest::Write { id, data } => match self.procs.lock().unwrap().get(&id) {
                Some(p) if p.input.send(data.into_bytes()).is_ok() => HostResponse::ok(),
                _ => HostResponse::err("session not running"),
            },
            HostRequest::Resize { id, cols, rows } => {
                if let Some(p) = self.procs.lock().unwrap().get(&id) {
                    let _ = p.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                }
                HostResponse::ok()
            }
            HostRequest::Kill { id } => {
                self.kill(&id);
                HostResponse::ok()
            }
            HostRequest::List => {
                let terms = self
                    .procs
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(id, p)| Term { id: id.clone(), pid: p.pid, fg: p.master.process_group_leader() })
                    .collect();
                HostResponse { terms: Some(terms), ..HostResponse::ok() }
            }
            HostRequest::Attach { .. } | HostRequest::Quit => HostResponse::err("not a control request"),
        }
    }

    /// Replay and live output under one lock: nothing missed, nothing twice.
    fn attach(&self, id: &str, mut conn: UnixStream) -> io::Result<()> {
        let out = self.procs.lock().unwrap().get(id).map(|p| p.out.clone());
        let Some(out) = out else {
            return reply(&mut conn, &HostResponse::err("session not running"));
        };
        let mut out = out.lock().unwrap();
        reply(&mut conn, &HostResponse::ok())?;
        conn.write_all(&out.replay())?;
        // A client that stops reading must not stall the terminal for long.
        conn.set_write_timeout(Some(Duration::from_secs(2)))?;
        out.subs.push(conn);
        Ok(())
    }
}

fn reply(conn: &mut UnixStream, resp: &HostResponse) -> io::Result<()> {
    let mut line = serde_json::to_vec(resp)?;
    line.push(b'\n');
    conn.write_all(&line)
}

/// Runs the helper on `sock` until told to quit or, with `idle_exit`, until it has had no
/// terminals and no clients for a while. Blocks.
pub fn serve(sock: &Path, idle_exit: bool) -> io::Result<()> {
    if let Some(dir) = sock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(sock);
    let listener = UnixListener::bind(sock)?;
    let host = Arc::new(Host::default());
    if idle_exit {
        let (host, sock) = (host.clone(), sock.to_path_buf());
        thread::spawn(move || {
            let mut idle_since = Instant::now();
            loop {
                thread::sleep(Duration::from_millis(500));
                let busy = host.clients.load(Ordering::SeqCst) > 0 || !host.procs.lock().unwrap().is_empty();
                if busy {
                    idle_since = Instant::now();
                } else if idle_since.elapsed() >= IDLE_EXIT {
                    let _ = std::fs::remove_file(&sock);
                    std::process::exit(0);
                }
            }
        });
    }
    for conn in listener.incoming().flatten() {
        let host = host.clone();
        let sock = sock.to_path_buf();
        thread::spawn(move || {
            host.clients.fetch_add(1, Ordering::SeqCst);
            let _ = connection(&host, conn, &sock);
            host.clients.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

fn connection(host: &Arc<Host>, conn: UnixStream, sock: &Path) -> io::Result<()> {
    let mut out = conn.try_clone()?;
    for line in BufReader::new(conn).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<HostRequest>(&line) {
            Ok(HostRequest::Attach { id }) => return host.attach(&id, out),
            Ok(HostRequest::Quit) => {
                host.kill_all();
                let _ = reply(&mut out, &HostResponse::ok());
                let _ = std::fs::remove_file(sock);
                std::process::exit(0);
            }
            Ok(req) => reply(&mut out, &host.handle(req))?,
            Err(_) => reply(&mut out, &HostResponse::err("unknown request"))?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_buffer_replays_from_a_line_break() {
        let mut o = Out::default();
        o.push(b"first line\nsecond");
        assert_eq!(o.replay(), b"first line\nsecond");
        o.push(&vec![b'x'; RING]);
        assert!(o.cut);
        assert!(o.replay().is_empty(), "no line break left: nothing safe to replay");
        o.push(b"\nafter");
        assert_eq!(o.replay(), b"after");
    }

    #[test]
    fn unknown_requests_get_an_error_not_a_hangup() {
        // an older helper meeting a newer app must answer, not drop the connection
        assert!(serde_json::from_str::<HostRequest>(r#"{"op":"from_the_future"}"#).is_err());
        let r = HostResponse::err("unknown request");
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"ok":false,"error":"unknown request"}"#);
    }
}
