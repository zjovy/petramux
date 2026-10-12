use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
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
pub fn run_with_timeout(cmd: Command, timeout: Duration) -> Option<Vec<u8>> {
    run(cmd, timeout).map(|(_, out)| out)
}

/// Like `run_with_timeout`, but a failed exit is `None` too.
pub fn run_checked(cmd: Command, timeout: Duration) -> Option<Vec<u8>> {
    run(cmd, timeout).filter(|(status, _)| status.success()).map(|(_, out)| out)
}

fn run(mut cmd: Command, timeout: Duration) -> Option<(ExitStatus, Vec<u8>)> {
    let mut child = cmd
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
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    // A background job left holding stdout open must not hang us.
    rx.recv_timeout(Duration::from_secs(1)).ok().map(|out| (status, out))
}

/// Launchd-like starting env: just the identity vars, none of the launcher's session state.
/// Roost started by `open` from a Claude session would otherwise pass that session's
/// `CLAUDE_CODE_SESSION_ID` (and terminal vars) on to every Claude it launches.
pub fn clean_base(env: &Env) -> Env {
    // SSH_AUTH_SOCK comes from launchd (macOS ssh-agent) and no login shell recreates it.
    let mut base: Env = ["HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "SSH_AUTH_SOCK", "__CF_USER_TEXT_ENCODING"]
        .iter()
        .filter_map(|k| env.get(*k).map(|v| (k.to_string(), v.clone())))
        .collect();
    base.insert("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into());
    base
}

/// Env of a login shell started from exactly `base` (nothing inherited from this process).
pub fn login_env(shell: &str, base: &Env) -> Option<Env> {
    let mut cmd = Command::new(shell);
    cmd.args(["-lic", &format!("printf {MARKER}; env -0")]).env_clear().envs(base);
    if let Some(home) = base.get("HOME") {
        cmd.current_dir(home);
    }
    run_with_timeout(cmd, Duration::from_secs(5)).and_then(|out| parse_env(&out))
}

/// The user's login-shell env (apps opened from Finder don't get it), with a fallback.
pub fn capture_login_env() -> Env {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let own: Env = std::env::vars().collect();
    login_env(&shell, &clean_base(&own)).unwrap_or_else(|| fallback_env(&own))
}

fn fallback_env(own: &Env) -> Env {
    let mut env = clean_base(own);
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
    // `roost` must be callable from sessions even when ~/.local/bin isn't on the user's PATH.
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.display().to_string())) {
        let path = env.get("PATH").map(|p| format!("{dir}:{p}")).unwrap_or(dir);
        env.insert("PATH".into(), path);
    }
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
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "echo noise; printf __ROOST_ENV__; env -0"]);
        let out = run_with_timeout(cmd, Duration::from_secs(5)).unwrap();
        assert!(parse_env(&out).unwrap().contains_key("PATH"));
    }

    #[test]
    fn slow_command_times_out() {
        let start = Instant::now();
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("5");
        assert_eq!(run_with_timeout(cmd, Duration::from_millis(200)), None);
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
        assert!(env["PATH"].ends_with(":/bin"));
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

    #[test]
    fn clean_base_keeps_identity_and_drops_launcher_session_vars() {
        let mut env = Env::new();
        for (k, v) in [("HOME", "/h"), ("USER", "me"), ("SHELL", "/bin/zsh"), ("PATH", "/x"), ("SSH_AUTH_SOCK", "/a"),
                       ("CLAUDE_CODE_SESSION_ID", "s"), ("CMUX_SOCKET", "c"), ("TERM_PROGRAM", "cmux")] {
            env.insert(k.into(), v.into());
        }
        let base = clean_base(&env);
        assert_eq!(
            base.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["HOME", "PATH", "SHELL", "SSH_AUTH_SOCK", "USER"]
        );
        assert_eq!(base["PATH"], "/usr/bin:/bin:/usr/sbin:/sbin");
    }

    #[test]
    fn login_env_inherits_nothing_from_this_process() {
        let dir = tempfile::tempdir().unwrap();
        let mut base = Env::new();
        base.insert("HOME".into(), dir.path().display().to_string());
        base.insert("PATH".into(), "/usr/bin:/bin".into());
        let env = login_env("/bin/sh", &base).unwrap();
        assert_eq!(env["HOME"], dir.path().display().to_string());
        assert!(std::env::var("CARGO_PKG_NAME").is_ok(), "cargo test sets this in our process");
        assert!(!env.contains_key("CARGO_PKG_NAME"));
    }

    #[test]
    fn fallback_env_starts_clean_too() {
        let mut own = Env::new();
        for (k, v) in [("HOME", "/h"), ("PATH", "/x"), ("CLAUDE_CODE_SESSION_ID", "s"), ("CMUX_SOCKET", "c")] {
            own.insert(k.into(), v.into());
        }
        let env = fallback_env(&own);
        assert!(!env.contains_key("CLAUDE_CODE_SESSION_ID") && !env.contains_key("CMUX_SOCKET"));
        assert_eq!(env["PATH"], "/opt/homebrew/bin:/usr/local/bin:/h/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin");
    }

    #[test]
    fn sessions_can_always_find_the_roost_cli() {
        let mut base = Env::new();
        base.insert("PATH".into(), "/bin".into());
        let env = child_env(&base, "abc", Path::new("/tmp/r.sock"));
        let bin_dir = std::env::current_exe().unwrap().parent().unwrap().display().to_string();
        assert_eq!(env["PATH"], format!("{bin_dir}:/bin"));
    }
}
