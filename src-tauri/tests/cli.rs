use std::io::Write;
use std::process::{Command, Stdio};

fn roost(args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_petramux"))
        .args(args)
        .env("ROOST_SOCK", "/tmp/roost-test-none.sock")
        .env("ROOST_SESSION", "x")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{}").unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn hook_exits_zero_and_silent_without_app() {
    let out = roost(&["hook", "Stop"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn ls_without_app_says_so() {
    let out = roost(&["ls"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "petramux isn't running");
}
