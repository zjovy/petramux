use crate::state::Status;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    New {
        prompt: String,
        cwd: String,
        #[serde(default)]
        worktree: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        parent: Option<String>,
    },
    Ls,
    Send {
        target: String,
        text: String,
        #[serde(default)]
        from: Option<String>,
    },
    Stop {
        target: String,
    },
    Hook {
        session: String,
        event: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LsRow {
    pub id: String,
    pub name: String,
    pub status: Status,
    pub repo: String,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<Vec<LsRow>>,
}

impl Response {
    pub fn ok() -> Self {
        Self { ok: true, ..Default::default() }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()), ..Default::default() }
    }
}

pub type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// Binds the socket (replacing any stale file) and serves each connection on a thread. Only
/// petramux talks to it: this program, or `also` (see `peer::trusted`).
pub fn serve(path: &Path, handler: Handler, also: Option<PathBuf>) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            if !crate::peer::trusted(&conn, also.as_deref()) {
                let _ = writeln!(&conn, "{}", serde_json::json!(Response::err("not petramux")));
                continue;
            }
            let handler = handler.clone();
            thread::spawn(move || {
                let _ = handle_conn(conn, handler);
            });
        }
    });
    Ok(())
}

fn handle_conn(conn: UnixStream, handler: Handler) -> io::Result<()> {
    let mut out = conn.try_clone()?;
    for line in BufReader::new(conn).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handler(req),
            Err(e) => Response::err(format!("bad request: {e}")),
        };
        serde_json::to_writer(&mut out, &resp)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

/// One request, one response; `timeout` bounds both the write and the read.
pub fn call(path: &Path, req: &Request, timeout: Duration) -> io::Result<Response> {
    let mut conn = UnixStream::connect(path)?;
    conn.set_read_timeout(Some(timeout))?;
    conn.set_write_timeout(Some(timeout))?;
    let mut line = serde_json::to_vec(req)?;
    line.push(b'\n');
    conn.write_all(&line)?;
    let mut resp = String::new();
    BufReader::new(conn).read_line(&mut resp)?;
    Ok(serde_json::from_str(&resp)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_format_matches_spec() {
        assert_eq!(serde_json::to_value(&Request::Ls).unwrap(), json!({"op": "ls"}));
        let new: Request = serde_json::from_value(json!({
            "op": "new", "prompt": "p", "cwd": "/r", "worktree": null, "name": null, "parent": null
        }))
        .unwrap();
        assert_eq!(
            new,
            Request::New { prompt: "p".into(), cwd: "/r".into(), worktree: None, name: None, parent: None }
        );
        assert_eq!(serde_json::to_value(Response::ok()).unwrap(), json!({"ok": true}));
    }

    fn echo_handler() -> Handler {
        Arc::new(|req| match req {
            Request::Ls => Response { sessions: Some(vec![]), ..Response::ok() },
            Request::Stop { target } => Response::err(format!("no session named {target}")),
            _ => Response::ok(),
        })
    }

    #[test]
    fn round_trip_over_a_real_socket_replacing_a_stale_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roost.sock");
        std::fs::write(&path, "stale").unwrap();
        serve(&path, echo_handler(), None).unwrap();
        let t = Duration::from_secs(2);
        let ls = call(&path, &Request::Ls, t).unwrap();
        assert_eq!(ls.sessions, Some(vec![]));
        let stop = call(&path, &Request::Stop { target: "x".into() }, t).unwrap();
        assert_eq!(stop, Response::err("no session named x"));
    }

    #[test]
    fn another_program_is_turned_away() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roost.sock");
        serve(&path, echo_handler(), None).unwrap();
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf '{{\"op\":\"ls\"}}\\n' | /usr/bin/nc -U '{}'", path.display()))
            .output()
            .unwrap();
        let resp: Response = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(resp, Response::err("not petramux"));
    }

    #[test]
    fn garbage_gets_an_error_response() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roost.sock");
        serve(&path, echo_handler(), None).unwrap();
        let mut conn = UnixStream::connect(&path).unwrap();
        conn.write_all(b"garbage\n").unwrap();
        let mut line = String::new();
        BufReader::new(conn).read_line(&mut line).unwrap();
        let resp: Response = serde_json::from_str(&line).unwrap();
        assert!(!resp.ok);
        assert!(resp.error.unwrap().starts_with("bad request"));
    }

    #[test]
    fn call_without_server_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(call(&dir.path().join("none.sock"), &Request::Ls, Duration::from_millis(200)).is_err());
    }
}
