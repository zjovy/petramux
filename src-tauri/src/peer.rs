//! Who is on the other end of a socket: petramux's sockets only serve petramux itself.
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// The connected process's pid (macOS `LOCAL_PEERPID`).
fn peer_pid(conn: &UnixStream) -> Option<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(conn.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEERPID, &mut pid as *mut _ as *mut libc::c_void, &mut len)
    };
    (rc == 0 && pid > 0).then_some(pid)
}

/// The program a process runs.
fn pid_exe(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    (n > 0).then(|| PathBuf::from(String::from_utf8_lossy(&buf[..n as usize]).into_owned()))
}

fn same(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A client may talk to this socket when it's this very process, or runs this program or `also`
/// (the app's helper and its hooks run the app's own binary; tests run a separate one).
pub fn trusted(conn: &UnixStream, also: Option<&Path>) -> bool {
    let Some(pid) = peer_pid(conn) else { return false };
    if pid == std::process::id() as i32 {
        return true;
    }
    let Some(exe) = pid_exe(pid) else { return false };
    let me = std::env::current_exe().ok();
    me.iter().map(PathBuf::as_path).chain(also).any(|p| same(&exe, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::process::Command;

    #[test]
    fn only_this_program_is_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let listener = UnixListener::bind(&path).unwrap();

        // this process
        let _me = UnixStream::connect(&path).unwrap();
        let (conn, _) = listener.accept().unwrap();
        assert!(trusted(&conn, None));

        // another program (nc), unless it's the one named
        let mut nc = Command::new("/usr/bin/nc").arg("-U").arg(&path).stdin(std::process::Stdio::piped()).spawn().unwrap();
        let (conn, _) = listener.accept().unwrap();
        assert!(!trusted(&conn, None));
        assert!(!trusted(&conn, Some(Path::new("/bin/sh"))));
        assert!(trusted(&conn, Some(Path::new("/usr/bin/nc"))));
        let _ = nc.kill();
        let _ = nc.wait();
    }
}
