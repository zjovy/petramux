//! The terminal helper (`petramux pty-host <socket>`): owns every session's PTY so sessions
//! keep running when the app quits or updates. The app is a client that reconnects.
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
    /// The last on/off of the modes a late client needs (bracketed paste, alternate screen, mouse,
    /// focus, cursor), so a replay that starts after a cut still sets them.
    modes: BTreeMap<u32, bool>,
    /// The last window title set (OSC 0/2): it names what a Claude pane shows.
    title: Option<Vec<u8>>,
}

/// The last complete `ESC ] 0;` / `ESC ] 2;` title in `bytes`, with its terminator.
fn last_title(bytes: &[u8]) -> Option<Vec<u8>> {
    let start = bytes.windows(4).rposition(|w| w == b"\x1b]0;" || w == b"\x1b]2;")?;
    let rest = &bytes[start..];
    let end = rest.iter().position(|&c| c == 0x07).map(|i| i + 1).or_else(|| rest.windows(2).position(|w| w == b"\x1b\\").map(|i| i + 2))?;
    Some(rest[..end].to_vec())
}

const MODES: [u32; 10] = [25, 47, 1000, 1002, 1003, 1004, 1006, 1047, 1049, 2004];

/// `ESC [ ? n ; m h` / `l` settings in `bytes`, in order.
fn mode_sets(bytes: &[u8]) -> Vec<(u32, bool)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = bytes[i..].windows(3).position(|w| w == b"\x1b[?") {
        let start = i + p + 3;
        let mut j = start;
        while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
            j += 1;
        }
        if j < bytes.len() && matches!(bytes[j], b'h' | b'l') {
            for n in bytes[start..j].split(|&c| c == b';') {
                if let Some(n) = std::str::from_utf8(n).ok().and_then(|s| s.parse().ok()) {
                    out.push((n, bytes[j] == b'h'));
                }
            }
        }
        i = j.max(start);
    }
    out
}

impl Out {
    fn push(&mut self, bytes: &[u8]) {
        for (mode, on) in mode_sets(bytes) {
            if MODES.contains(&mode) {
                self.modes.insert(mode, on);
            }
        }
        if let Some(t) = last_title(bytes) {
            self.title = Some(t);
        }
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
        let mut replay: Vec<u8> = self.modes.iter().flat_map(|(m, on)| format!("\x1b[?{m}{}", if *on { 'h' } else { 'l' }).into_bytes()).collect();
        replay.extend(self.title.iter().flatten());
        // start at a clean boundary: a line break, or the escape that begins the next sequence
        // (a TUI redraws with cursor moves and hardly ever prints a line break)
        if let Some(i) = all.iter().position(|&c| c == b'\n' || c == 0x1b) {
            replay.extend_from_slice(&all[i + usize::from(all[i] == b'\n')..]);
        }
        replay
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
        // A client that stops reading must not stall the terminal for long, replay included.
        conn.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut out = out.lock().unwrap();
        reply(&mut conn, &HostResponse::ok())?;
        conn.write_all(&out.replay())?;
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
/// The file where the app names its program (see `pty::name_client`).
pub fn client_file(sock: &Path) -> PathBuf {
    sock.with_file_name("pty-client")
}

/// Only petramux talks to it: this program, or the app's (see `peer::trusted`).
pub fn serve(sock: &Path, idle_exit: bool) -> io::Result<()> {
    if let Some(dir) = sock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if UnixStream::connect(sock).is_ok() {
        return Ok(()); // another helper already holds the terminals
    }
    let _ = std::fs::remove_file(sock);
    let listener = UnixListener::bind(sock)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))?;
    }
    let host = Arc::new(Host::default());
    if idle_exit {
        let (host, sock) = (host.clone(), sock.to_path_buf());
        thread::spawn(move || {
            let mut idle_since = Instant::now();
            loop {
                thread::sleep(Duration::from_millis(500));
                // Its socket gone (folder deleted, a test's temp dir dropped): nothing can ever
                // reach these terminals again, so end them rather than leave them running.
                if !sock.exists() {
                    host.kill_all();
                    std::process::exit(0);
                }
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
        let app = std::fs::read_to_string(client_file(sock)).ok().map(PathBuf::from);
        if !crate::peer::trusted(&conn, app.as_deref()) {
            continue; // dropped: it can neither read nor type into any terminal
        }
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
    fn a_cut_replay_first_restores_the_terminal_modes_set_before_the_cut() {
        let mut o = Out::default();
        o.push(b"\x1b[?2004h\x1b[?1049h\x1b[?25l");
        o.push(b"\x1b[?25h");
        o.push(&vec![b'x'; RING]);
        o.push(b"\nend");
        let r = String::from_utf8(o.replay()).unwrap();
        assert!(r.ends_with("end"), "{r:?}");
        let modes = &r[..r.len() - 3];
        for m in ["\x1b[?2004h", "\x1b[?1049h", "\x1b[?25h"] {
            assert!(modes.contains(m), "{m:?} missing from {modes:?}");
        }
        assert!(!modes.contains("\x1b[?25l"), "the last setting wins");
        // without a cut, the modes are already in the replay itself
        let mut whole = Out::default();
        whole.push(b"\x1b[?2004hhi");
        assert_eq!(whole.replay(), b"\x1b[?2004hhi");
    }

    #[test]
    fn a_cut_replay_keeps_the_last_title_and_starts_at_a_clean_sequence() {
        let mut o = Out::default();
        o.push(b"\x1b]0;\xe2\x97\x90 task-3-impl\x07");
        // a TUI redraws with cursor moves, hardly ever a line break
        o.push(&b"\x1b[2Gx\x1b[3Gy".repeat(RING / 8));
        o.push(b"\x1b[5Gend");
        assert!(o.cut);
        let r = o.replay();
        let text = String::from_utf8_lossy(&r);
        assert!(text.contains("\x1b]0;◐ task-3-impl\x07"), "the title set before the cut comes first");
        assert!(r.len() > RING / 2, "most of the screen is kept: {} bytes", r.len());
        assert!(text.ends_with("\x1b[5Gend"));
    }

    #[test]
    fn another_program_gets_nothing_from_the_helper() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("pty.sock");
        let s1 = sock.clone();
        thread::spawn(move || serve(&s1, false));
        let end = Instant::now() + Duration::from_secs(3);
        while UnixStream::connect(&sock).is_err() && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        let ask = |program: &str| {
            let script = format!("printf '%s\\n' '{}' | {program} -U '{}'", serde_json::to_string(&HostRequest::List).unwrap(), sock.display());
            let out = std::process::Command::new("/bin/sh").arg("-c").arg(script).output().unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        assert_eq!(ask("/usr/bin/nc -w 1"), "", "dropped without a word");
        // the app, wherever it runs from now (moved, renamed), names itself next to the socket
        crate::pty::name_client(&sock, Path::new("/usr/bin/nc")).unwrap();
        assert!(ask("/usr/bin/nc -w 1").contains("\"ok\":true"), "answered once named");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(client_file(&sock)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "only you can name it");
    }

    #[test]
    fn a_second_helper_leaves_the_first_one_alone() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("pty.sock");
        let s1 = sock.clone();
        thread::spawn(move || serve(&s1, false));
        let end = Instant::now() + Duration::from_secs(3);
        while UnixStream::connect(&sock).is_err() && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(serve(&sock, false).is_ok(), "returns at once instead of taking the socket");
        assert!(UnixStream::connect(&sock).is_ok(), "the first helper still answers");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn unknown_requests_get_an_error_not_a_hangup() {
        // an older helper meeting a newer app must answer, not drop the connection
        assert!(serde_json::from_str::<HostRequest>(r#"{"op":"from_the_future"}"#).is_err());
        let r = HostResponse::err("unknown request");
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"ok":false,"error":"unknown request"}"#);
    }
}
