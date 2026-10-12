use crate::engine::{self, Attention, Core, NewSession, Sink, View};
use crate::past::Past;
use crate::paths::Paths;
use crate::state::Status;
use crate::{shellenv, sock};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent, Wry};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;

const PENDING_CAP: usize = 1 << 20;

struct UiSink {
    app: AppHandle,
    // Lock order: pending, then channels.
    pending: Mutex<HashMap<String, String>>,
    channels: Mutex<HashMap<String, Channel<String>>>,
    selected: Mutex<Option<String>>,
}

impl UiSink {
    fn attach(&self, id: String, ch: Channel<String>) {
        let mut pending = self.pending.lock().unwrap();
        if let Some(buf) = pending.remove(&id) {
            let _ = ch.send(buf);
        }
        self.channels.lock().unwrap().insert(id, ch);
    }

    fn detach(&self, id: &str) {
        self.pending.lock().unwrap().remove(id);
        self.channels.lock().unwrap().remove(id);
    }

    fn badge(&self, views: &[View]) {
        let n = views.iter().filter(|v| v.status == Status::NeedsYou).count() as i64;
        if let Some(w) = self.app.get_webview_window("main") {
            let _ = w.set_badge_count(if n > 0 { Some(n) } else { None });
        }
    }
}

impl Sink for UiSink {
    fn sessions_changed(&self, views: Vec<View>) {
        self.badge(&views);
        let _ = self.app.emit("sessions_changed", views);
    }

    fn session_changed(&self, view: View) {
        let _ = self.app.emit("session_changed", &view);
        if let Some(core) = self.app.try_state::<Arc<Core>>() {
            self.badge(&core.views());
        }
    }

    fn output(&self, id: &str, text: String) {
        let mut pending = self.pending.lock().unwrap();
        if let Some(ch) = self.channels.lock().unwrap().get(id) {
            if ch.send(text.clone()).is_ok() {
                return;
            }
        }
        let buf = pending.entry(id.to_string()).or_default();
        buf.push_str(&text);
        if buf.len() > PENDING_CAP {
            let from = buf.len() - PENDING_CAP;
            let cut = (from..buf.len()).find(|i| buf.is_char_boundary(*i)).unwrap_or(buf.len());
            buf.drain(..cut);
        }
    }

    fn attention(&self, view: &View, kind: Attention) {
        let focused = self
            .app
            .get_webview_window("main")
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(false);
        let selected = self.selected.lock().unwrap().as_deref() == Some(view.id.as_str());
        if focused && selected {
            return;
        }
        let body = match kind {
            Attention::NeedsYou => format!("{} needs you", view.name),
            Attention::Done => format!("{} is done", view.name),
        };
        let _ = self.app.notification().builder().title("Roost").body(body).show();
    }
}

type CoreState<'a> = State<'a, Arc<Core>>;
type SinkState<'a> = State<'a, Arc<UiSink>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Boot {
    sessions: Vec<View>,
    recent_repos: Vec<String>,
    claude_found: bool,
}

#[tauri::command]
fn boot(core: CoreState) -> Boot {
    Boot { sessions: core.views(), recent_repos: core.recent_repos(), claude_found: core.claude_found() }
}

#[tauri::command]
fn attach(id: String, output: Channel<String>, sink: SinkState) {
    sink.attach(id, output);
}

#[tauri::command]
fn select_session(id: String, core: CoreState, sink: SinkState) -> Result<View, String> {
    *sink.selected.lock().unwrap() = Some(id.clone());
    core.select(&id)
}

#[tauri::command]
fn new_session(prompt: String, repo: String, worktree: bool, core: CoreState) -> Result<View, String> {
    core.new_session(NewSession { prompt, repo, worktree, name: None, parent: None })
}

// async: a session that stops reading may block this write; never on the main thread.
#[tauri::command]
async fn write(id: String, data: String, core: CoreState<'_>) -> Result<(), String> {
    core.write(&id, data.as_bytes())
}

#[tauri::command]
fn resize(id: String, cols: u16, rows: u16, core: CoreState) {
    core.resize(&id, cols, rows);
}

#[tauri::command]
fn rename(id: String, name: String, core: CoreState) -> Result<View, String> {
    core.rename(&id, &name)
}

#[tauri::command]
fn close(id: String, core: CoreState, sink: SinkState) {
    core.close(&id);
    sink.detach(&id);
}

#[tauri::command]
fn start_fresh(id: String, core: CoreState, sink: SinkState) -> Result<View, String> {
    sink.detach(&id);
    core.start_fresh(&id)
}

#[tauri::command]
async fn list_past(repo: String, core: CoreState<'_>) -> Result<Vec<Past>, String> {
    Ok(core.list_past(&repo))
}

#[tauri::command]
fn resume_past(repo: String, claude_id: String, title: String, workdir: String, core: CoreState) -> Result<View, String> {
    core.resume_past(&repo, Past { claude_id, title, workdir, modified_ms: 0 })
}

// async: the blocking dialog must not run on the main thread.
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    app.dialog().file().blocking_pick_folder().map(|p| p.to_string())
}

#[tauri::command]
async fn is_git_repo(path: String) -> bool {
    engine::is_git_repo(&path)
}

#[tauri::command]
async fn ask(app: AppHandle, message: String, ok: String) -> bool {
    app.dialog()
        .message(message)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(ok, "Cancel".into()))
        .blocking_show()
}

fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let quit = MenuItem::with_id(app, "quit", "Quit Roost", true, Some("CmdOrCtrl+Q"))?;
    let roost = Submenu::with_items(app, "Roost", true, &[
        &PredefinedMenuItem::about(app, None, None)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::hide(app, None)?,
        &PredefinedMenuItem::hide_others(app, None)?,
        &PredefinedMenuItem::separator(app)?,
        &quit,
    ])?;
    // Copy/paste only work in a macOS webview when the Edit menu has them.
    let edit = Submenu::with_items(app, "Edit", true, &[
        &PredefinedMenuItem::undo(app, None)?,
        &PredefinedMenuItem::redo(app, None)?,
        &PredefinedMenuItem::separator(app)?,
        &PredefinedMenuItem::cut(app, None)?,
        &PredefinedMenuItem::copy(app, None)?,
        &PredefinedMenuItem::paste(app, None)?,
        &PredefinedMenuItem::select_all(app, None)?,
    ])?;
    // No "Close Window" item: ⌘W belongs to the UI (close session).
    let window = Submenu::with_items(app, "Window", true, &[
        &PredefinedMenuItem::minimize(app, None)?,
        &PredefinedMenuItem::fullscreen(app, None)?,
    ])?;
    Menu::with_items(app, &[&roost, &edit, &window])
}

/// Quit, confirming first if any agent is mid-turn.
fn quit_guarded(app: &AppHandle) {
    let core = app.state::<Arc<Core>>().inner().clone();
    let working = core.views().iter().filter(|v| v.status == Status::Working).count();
    let handle = app.clone();
    let go = move || {
        core.shutdown();
        handle.exit(0);
    };
    if working == 0 {
        return go();
    }
    let msg = if working == 1 {
        "1 agent is working. Quit anyway?".to_string()
    } else {
        format!("{working} agents are working. Quit anyway?")
    };
    app.dialog()
        .message(msg)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom("Quit".into(), "Cancel".into()))
        .show(move |yes| {
            if yes {
                go()
            }
        });
}

/// Keeps `~/.local/bin/roost` pointing at this binary without clobbering anything else.
fn install_cli_link(exe: &Path) {
    let Some(home) = std::env::var_os("HOME") else { return };
    let link = Path::new(&home).join(".local/bin/roost");
    match std::fs::read_link(&link) {
        Ok(target) if target == exe => return,
        Ok(target) if target.file_name().is_some_and(|n| n == "roost") => {
            let _ = std::fs::remove_file(&link);
        }
        Ok(_) => {
            eprintln!("roost: ~/.local/bin/roost points somewhere else; leaving it");
            return;
        }
        Err(_) if link.symlink_metadata().is_ok() => {
            eprintln!("roost: ~/.local/bin/roost exists and isn't a symlink; leaving it");
            return;
        }
        Err(_) => {}
    }
    let _ = std::fs::create_dir_all(link.parent().unwrap());
    if let Err(e) = std::os::unix::fs::symlink(exe, &link) {
        eprintln!("roost: could not link the CLI: {e}");
    }
}

pub fn run_app() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .menu(menu)
        .on_menu_event(|app, e| {
            if e.id() == "quit" {
                quit_guarded(app);
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let paths = Paths::new(Paths::default_dir());
            let exe = std::env::current_exe()?;
            crate::integration::install(&paths, &exe)?;
            install_cli_link(&exe);
            // ponytail: blocks startup while the login shell runs (5 s worst case); move to a thread if it's noticeable.
            let env = shellenv::capture_login_env();
            let claude = shellenv::find_claude(&env, std::env::var("ROOST_CLAUDE_BIN").ok());
            let sink = Arc::new(UiSink {
                app: handle,
                pending: Default::default(),
                channels: Default::default(),
                selected: Default::default(),
            });
            let core = Core::new(paths.clone(), env, claude, sink.clone());
            sock::serve(&paths.sock(), core.handler())?;
            app.manage(core);
            app.manage(sink);
            Ok(())
        })
        .on_window_event(|w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                quit_guarded(w.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            boot, attach, select_session, new_session, write, resize, rename, close,
            start_fresh, list_past, resume_past, pick_folder, is_git_repo, ask
        ])
        .build(tauri::generate_context!())
        .expect("error while building Roost")
        .run(|app, event| match event {
            // `code: None` = the last window went away without our exit(0); `Some` = our own exit(0).
            RunEvent::ExitRequested { api, code: None, .. } => {
                api.prevent_exit();
                quit_guarded(app);
            }
            // Dock Quit, logout and shutdown arrive here directly (tao has no
            // applicationShouldTerminate:), so they can't be confirmed; at least stop cleanly.
            RunEvent::Exit => {
                if let Some(core) = app.try_state::<Arc<Core>>() {
                    core.shutdown();
                }
            }
            _ => {}
        });
}
