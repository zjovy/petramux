use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub type Env = BTreeMap<String, String>;
pub const MARKER: &str = "__ROOST_ENV__";

/// Parses `env -0` output after MARKER; anything before it is shell startup noise.
pub fn parse_env(out: &[u8]) -> Option<Env> {
    let text = String::from_utf8_lossy(out);
    let (_, rest) = text.split_once(MARKER)?;
    Some(
        rest.split('\0')
            .filter_map(|kv| kv.split_once('='))
            .filter(|(k, _)| !k.is_empty())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

/// Runs a command, returns stdout, or None if it fails or outlives `timeout`.
pub fn run_with_timeout(prog: &str, args: &[&str], timeout: Duration) -> Option<Vec<u8>> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    // A background job left holding stdout open must not hang us.
    rx.recv_timeout(Duration::from_secs(1)).ok()
}

/// The user's login-shell env (apps opened from Finder don't get it), with a fallback.
pub fn capture_login_env() -> Env {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let script = format!("printf {MARKER}; env -0");
    run_with_timeout(&shell, &["-lic", &script], Duration::from_secs(5))
        .and_then(|out| parse_env(&out))
        .unwrap_or_else(fallback_env)
}

fn fallback_env() -> Env {
    let mut env: Env = std::env::vars().collect();
    let home = env.get("HOME").cloned().unwrap_or_default();
    let path = env.get("PATH").cloned().unwrap_or_default();
    env.insert(
        "PATH".into(),
        format!("/opt/homebrew/bin:/usr/local/bin:{home}/.local/bin:{path}"),
    );
    env
}

pub fn find_claude(env: &Env, override_bin: Option<String>) -> Option<PathBuf> {
    if let Some(bin) = override_bin {
        return Some(PathBuf::from(bin));
    }
    env.get("PATH")?
        .split(':')
        .map(|dir| Path::new(dir).join("claude"))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Env for a session's `claude`: login env without nested-Claude markers, plus Roost vars.
pub fn child_env(base: &Env, session_id: &str, sock: &Path) -> Env {
    let mut env = base.clone();
    env.remove("CLAUDECODE");
    env.remove("CLAUDE_CODE_ENTRYPOINT");
    env.insert("ROOST_SESSION".into(), session_id.into());
    env.insert("ROOST_SOCK".into(), sock.display().to_string());
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parses_after_marker_ignoring_shell_noise() {
        let out = b"\x1b[32mwelcome!\x1b[0m\nPATH=/fake\n__ROOST_ENV__HOME=/Users/me\0PATH=/bin:/usr/bin\0EMPTY=\0";
        let env = parse_env(out).unwrap();
        assert_eq!(env.get("PATH").unwrap(), "/bin:/usr/bin");
        assert_eq!(env.get("HOME").unwrap(), "/Users/me");
        assert_eq!(env.get("EMPTY").unwrap(), "");
    }

    #[test]
    fn no_marker_means_no_env() {
        assert_eq!(parse_env(b"PATH=/bin\0"), None);
    }

    #[test]
    fn runs_a_real_shell_and_parses() {
        let out = run_with_timeout(
            "/bin/sh",
            &["-c", "echo noise; printf __ROOST_ENV__; env -0"],
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(parse_env(&out).unwrap().contains_key("PATH"));
    }

    #[test]
    fn slow_command_times_out() {
        let start = Instant::now();
        assert_eq!(run_with_timeout("/bin/sleep", &["5"], Duration::from_millis(200)), None);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn child_env_strips_nested_claude_markers_and_adds_roost_vars() {
        let mut base = Env::new();
        base.insert("CLAUDECODE".into(), "1".into());
        base.insert("CLAUDE_CODE_ENTRYPOINT".into(), "cli".into());
        base.insert("PATH".into(), "/bin".into());
        let env = child_env(&base, "abc", Path::new("/tmp/r.sock"));
        assert!(!env.contains_key("CLAUDECODE"));
        assert!(!env.contains_key("CLAUDE_CODE_ENTRYPOINT"));
        assert_eq!(env["ROOST_SESSION"], "abc");
        assert_eq!(env["ROOST_SOCK"], "/tmp/r.sock");
        assert_eq!(env["TERM"], "xterm-256color");
        assert_eq!(env["COLORTERM"], "truecolor");
        assert_eq!(env["PATH"], "/bin");
    }

    #[test]
    fn finds_claude_on_path_or_override() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("claude");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut env = Env::new();
        env.insert("PATH".into(), format!("/nope:{}", dir.path().display()));
        assert_eq!(find_claude(&env, None), Some(bin));
        assert_eq!(find_claude(&env, Some("/x/fake".into())), Some(PathBuf::from("/x/fake")));
        env.insert("PATH".into(), "/nope".into());
        assert_eq!(find_claude(&env, None), None);
    }
}
