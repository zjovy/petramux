//! Updates, from two places: the latest GitHub release (downloaded in the background, checked
//! against the key in tauri.conf.json, and put in place of the app), and a newer petramux built in
//! this repo. Either installs with one click, and every session keeps running through it (they
//! live in the helper, not the app).
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// Where `bun run tauri build` puts the app: this repo, as it was built.
pub fn built() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/bundle/macos/petramux.app")
}

/// The .app a program runs from, if it runs from one.
pub fn app_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

fn exe_time(app: &Path) -> Option<SystemTime> {
    std::fs::metadata(app.join("Contents/MacOS/petramux")).and_then(|m| m.modified()).ok()
}

/// A finished build newer than the running app. A build still being written (changed in the last
/// few seconds) waits.
pub fn newer(built: &Path, running: &Path, now: SystemTime) -> bool {
    if built == running || !built.join("Contents/Info.plist").exists() {
        return false;
    }
    match (exe_time(built), exe_time(running)) {
        (Some(b), Some(r)) => b > r && now.duration_since(b).is_ok_and(|age| age >= Duration::from_secs(5)),
        _ => false,
    }
}

/// A release was downloaded and put in place of the app: it runs from the next start.
static RELEASED: AtomicBool = AtomicBool::new(false);

pub fn release_installed() -> bool {
    RELEASED.load(Ordering::Relaxed)
}

/// Checks the latest GitHub release at start and every few hours. A newer one is downloaded,
/// verified, and swapped in for the app on disk; the app running now carries on until it restarts.
/// Only for an app installed as one (not `tauri dev`).
pub fn watch_releases(app: tauri::AppHandle) {
    if std::env::current_exe().ok().and_then(|e| app_of(&e)).is_none() {
        return;
    }
    std::thread::spawn(move || loop {
        if !release_installed() {
            match tauri::async_runtime::block_on(fetch_release(&app)) {
                Ok(true) => RELEASED.store(true, Ordering::Relaxed),
                Ok(false) => {}
                Err(e) => eprintln!("petramux: checking for an update: {e}"),
            }
        }
        std::thread::sleep(Duration::from_secs(4 * 3600));
    });
}

async fn fetch_release(app: &tauri::AppHandle) -> tauri_plugin_updater::Result<bool> {
    use tauri_plugin_updater::UpdaterExt;
    let Some(update) = app.updater()?.check().await? else { return Ok(false) };
    update.download_and_install(|_, _| {}, || {}).await?;
    Ok(true)
}

/// Waits for the app ($1) to quit, copies the build ($2, if any) over the app ($3), and opens it
/// with $4 (`open`).
const RESTART: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done
[ -n "$2" ] && /usr/bin/ditto "$2" "$3"
exec "$4" "$3""#;

/// The restart, as a process of its own that outlives the app.
pub fn restart(pid: u32, build: Option<&Path>, app: &Path, open: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(RESTART).arg("petramux-restart").arg(pid.to_string());
    cmd.arg(build.map(Path::as_os_str).unwrap_or_default()).arg(app).arg(open);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || child.wait()); // reaped if the app is still here when it ends
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn app(dir: &Path, name: &str, at: SystemTime) -> PathBuf {
        let app = dir.join(name);
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), "").unwrap();
        let exe = std::fs::File::create(app.join("Contents/MacOS/petramux")).unwrap();
        exe.set_modified(at).unwrap();
        app
    }

    #[test]
    fn a_finished_newer_build_is_an_update() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let ago = |s: u64| now - Duration::from_secs(s);
        let running = app(dir.path(), "running.app", ago(3600));
        assert!(newer(&app(dir.path(), "new.app", ago(60)), &running, now));
        assert!(!newer(&app(dir.path(), "old.app", ago(7200)), &running, now), "older");
        assert!(!newer(&app(dir.path(), "same.app", ago(3600)), &running, now), "the one installed");
        assert!(!newer(&app(dir.path(), "busy.app", ago(1)), &running, now), "still being written");
        assert!(!newer(&dir.path().join("none.app"), &running, now), "no build");
        assert!(!newer(&running, &running, now), "running the build itself");
        assert_eq!(app_of(Path::new("/A/petramux.app/Contents/MacOS/petramux")), Some(PathBuf::from("/A/petramux.app")));
        assert_eq!(app_of(Path::new("/r/target/debug/petramux")), None);
    }

    #[test]
    fn restart_waits_for_the_app_then_installs_and_opens() {
        let dir = tempfile::tempdir().unwrap();
        let build = dir.path().join("build.app");
        let installed = dir.path().join("installed.app");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::create_dir_all(&installed).unwrap();
        std::fs::write(build.join("v"), "2").unwrap();
        std::fs::write(installed.join("v"), "1").unwrap();
        // `open` stand-in: records what it opened
        let opened = dir.path().join("opened");
        let open = dir.path().join("open.sh");
        std::fs::write(&open, format!("#!/bin/sh\necho \"$1\" > '{}'\n", opened.display())).unwrap();
        std::fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let mut app_proc = Command::new("/bin/sleep").arg("1").spawn().unwrap();

        restart(app_proc.id(), Some(&build), &installed, &open).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(std::fs::read_to_string(installed.join("v")).unwrap(), "1", "not while the app runs");
        app_proc.wait().unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while !opened.exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(std::fs::read_to_string(installed.join("v")).unwrap(), "2", "installed");
        assert_eq!(std::fs::read_to_string(&opened).unwrap().trim(), installed.display().to_string(), "and opened");
    }
}
